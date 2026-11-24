#!/usr/bin/env python3
"""Offline self-test of check.py, the readiness check.

    selftest.py

Needs `git`, `yq` and `bash` on PATH; nothing else, and no network. It works
in a `tempfile.mkdtemp` directory with its own HOME and TMPDIR, no system or
global git config, a fixed identity and fixed dates, and runs `git init` on
a throwaway repository with no remote. Fixtures are commits built there with
git plumbing (`hash-object`, `update-index --index-info`, `write-tree`,
`commit-tree`); their config is written as JSON text at
.mintswarm/config.yaml. check.py runs as `sys.executable -I check.py ...`
with that repository as its working directory, its verdict written with
--json under the temporary directory, where a stale file is put first.

The ready fixture holds a complete config, .github/CODEOWNERS, AGENTS.md,
CLAUDE.md, CONTRIBUTING.md, docs/architecture/overview.md, a `docs`
component, tests/test_ok.py named by a gate, one stage source with its
rendered agent written by hand, the checkout's render-agents.py byte for
byte, and gates that call only python3 and shell builtins.

Checks:

- ready: every signal passes, the exit is 0, the verdict's keys, `tool`,
  `version`, `ref`, `config` and `verdict`, the 23 signal identifiers and
  their features in contract order, and the printed lines, which hold no
  temporary path;
- one mutation per signal and more (an overlap next to an uncovered file,
  an overlap with shared, a CODEOWNERS without an owner, an invalid login,
  a symlinked CLAUDE.md, an orphan, a missing and a symlinked rendered
  agent, a broken stage source, and gate commands outside the reader's
  grammar or running unnamed code: characters outside its set (`#`, `$`,
  quotes, `{` in a command word or an option's argument, `[`, `(`, `<`, a
  lone `>` or `&`), a redirection other than the four allowed, a trailing
  operator, digit words before a redirection, assignments before a command and through
  env, every class of builtin outside the allow-list (export, declare,
  typeset, local, readonly, read, unset, mapfile, let, `[[`, compgen,
  complete, bind, eval, trap, builtin, cd, coproc, set, printf), the
  prefixes env, exec, time and command, command words holding `=` (also
  `NAME+=`) or starting with `+`, an interpreter given an option of any
  kind (`-` or `+`) or no script, wrappers, find -exec, versioned and
  unmodelled interpreters, brace expansion, task runners, untracked
  scripts and a script that is a tracked symbolic link; when the wrapper or interpreter is not installed, gate-tools is
  expected to fail too): exit 1
  with exactly the expected set of failing signals, each named on its line
  with its feature and the expected reason text; variants that must stay
  ready (docs/CODEOWNERS, a symlinked CLAUDE.md, partial per-component
  plan limits with non_production globs, a gate with redirections
  and arguments after the script, a ref without render-agents.py, whose
  agents-drift passes as not applicable, the allowed builtins and source,
  the repository's own two gate commands, and a ref
  whose renderer differs from the checker's, which is noted) exit 0;
- script tools: the repository's four TypeScript gates, run with PATH
  holding only git, yq, bash, python3 and a stub pnpm, exit 0; without
  the stub pnpm they exit 1 failing only gate-tools, naming pnpm, which
  web-install.sh needs though no gate names it;
- no config: exit 1, every config signal fails naming its absence and the
  others pass;
- unusable input: no --ref, --ref twice, an unknown argument, --ref -x, a
  missing ref, an unborn HEAD, YAML yq rejects, YAML that is a list, a
  symlinked config, yq missing from PATH and a copy of check.py without
  ../plan-approval/floor.py exit 2 with one stderr line naming the cause, no
  stdout and no --json file, even a stale one;
- worktree: files in the worktree that differ from the ref leave the output
  unchanged;
- determinism: two runs on the ready fixture, a gaps fixture and a ref
  without a config give identical stdout and --json bytes;
- --run-gates: the ready fixture's gates pass and report gate:<name>
  signals after the static ones, twice with identical bytes; a failing
  gate reports `exit code 3`; a `sleep 30` gate with `timeout: 1` reports
  `timed out after 1 s` within seconds and leaves no process of its group;
  a file one gate writes is not in the next gate's worktree; gates get the
  environment the check was started with; a process a gate leaves in the
  background is killed when it exits; a gate that makes its worktree and
  its directory unwritable is still cleaned up; with GIT_DIR, GIT_WORK_TREE
  and GIT_INDEX_FILE set, a gate's git sees its own clean worktree at the
  ref and the checked repository's index is not written; with no gate to
  run a single
  gate-run signal fails; without the option no gate runs; SIGTERM during a
  gate exits 2 with no verdict and leaves no process, worktree or
  directory; after every run `git worktree list --porcelain` is what it was
  before and the check's TMPDIR is empty;
- standard library: every import of check.py and selftest.py is in a fixed
  allowlist of standard-library modules, and both parse as Python 3.9 with
  no `X | Y` annotations.

Prints one `ok` or `FAIL` line per check, then a count, and exits 0 only
when all passed.
"""

import ast
import copy
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CHECK = HERE / "check.py"
RENDERER = HERE.parent / "render-agents.py"
CONFIG = ".mintswarm/config.yaml"
RENDERER_PATH = ".mintswarm/dogfood/render-agents.py"
SYMLINK = "120000"
PIDFILE = "MSW_READINESS_PIDFILE"
GATE_OUT = "MSW_READINESS_OUT"
BACKGROUND = "MSW_READINESS_BGPID"

SIGNALS = [
    ("tracker", "tracker-kinds"),
    ("ingest", "trusted-ingest"),
    ("components", "components-cover"),
    ("review", "codeowners"),
    ("review", "reviewers"),
    ("agent-instructions", "agents-md"),
    ("agent-instructions", "claude-md"),
    ("agent-instructions", "contributing"),
    ("agent-instructions", "architecture-docs"),
    ("agent-instructions", "agents-drift"),
    ("gates", "gates-run"),
    ("gates", "gate-scripts-named"),
    ("gates", "gate-tools"),
    ("gates", "tests-gated"),
    ("gate-reports", "report-formats"),
    ("plan-approval", "plan-approval-config"),
    ("plan-approval", "docs-component"),
    ("plan-approval", "tests-exist"),
    ("plan-approval", "tests-declared"),
    ("plan-approval", "require-human-plan"),
    ("plan-approval", "assess-model"),
    ("landing", "landing-config"),
    ("landing", "require-human-landing"),
]
FEATURE = dict((name, feature) for feature, name in SIGNALS)
FILE_SIGNALS = {
    "codeowners",
    "agents-md",
    "claude-md",
    "contributing",
    "architecture-docs",
    "agents-drift",
    "tests-exist",
}
CONFIG_SIGNALS = {name for _, name in SIGNALS} - FILE_SIGNALS
VERDICT_KEYS = ["tool", "version", "ref", "config", "verdict", "signals"]
SIGNAL_KEYS = ["feature", "signal", "result", "reasons"]
STDLIB = frozenset(
    """
    argparse ast collections contextlib copy dataclasses datetime difflib errno fnmatch fractions functools hashlib
    importlib io itertools json math os pathlib re shlex shutil signal stat string subprocess sys tempfile textwrap
    time typing unicodedata uuid
    """.split()
)

AGENT_SOURCE = (
    b"---\n"
    b"name: probe\n"
    b"description: mintswarm probe stage. Reads the repository.\n"
    b"capabilities: [read, shell]\n"
    b"tier: standard\n"
    b"---\n"
    b"Probe instructions.\n"
)
AGENT_RENDERED = (
    b"---\n"
    b"# Generated from .mintswarm/stages/probe.md by .mintswarm/dogfood/render-agents.py; do not edit.\n"
    b"name: msw-probe\n"
    b'description: "mintswarm probe stage. Reads the repository."\n'
    b"tools: Read, Bash\n"
    b"model: sonnet\n"
    b"---\n"
    b"Probe instructions.\n"
)


class SelfTest:
    def __init__(self):
        self.failed = 0
        self.passed = 0

    def check(self, name: str, condition: bool, detail: str = "") -> bool:
        if condition:
            self.passed += 1
            print(f"ok   {name}")
        else:
            self.failed += 1
            print(f"FAIL {name}" + (f": {detail}" if detail else ""))
        return condition


T = SelfTest()


# --- fixtures ---------------------------------------------------------------


def ready_config() -> dict:
    return {
        "tracker": {
            "kind": "github",
            "kinds": {
                "source": "issue_types",
                "design": {"issue_type": "Design"},
                "plan": {"issue_type": "Plan"},
                "implementation": {"issue_type": "Task"},
            },
            "components": {"source": "labels", "label": "component:{name}"},
            "ingest": {"trusted": {"labels": ["mintswarm"], "authors": ["owner"]}, "match": "any"},
        },
        "components": {
            "agents": {
                "paths": [".claude/agents/**", ".mintswarm/stages.md", ".mintswarm/stages/**", ".mintswarm/dogfood/**"]
            },
            "code": {"paths": ["src/**", "tests/**"], "reviewers": ["@coder"]},
            "docs": {"paths": ["docs/**", "AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md", "README.md"]},
            "tooling": {"paths": [".github/**"]},
        },
        "shared": [CONFIG],
        "agents": {
            "default": {"harness": "claude", "model": "opus"},
            "stages": {"assess": {"model": "sonnet"}},
        },
        "pipeline": {
            "plan_approval": {
                "mode": "shadow",
                "require_human": {
                    "components": ["agents"],
                    "paths": [CONFIG, "tests/**", "AGENTS.md", "CLAUDE.md"],
                },
                "limits": {"files": 10, "lines": 300, "new_files": 3},
            },
            "landing": {
                "mode": "always",
                "require_human": {"paths": [".mintswarm/**", ".claude/**", "AGENTS.md", "CLAUDE.md"]},
                "footprint": {"lines_factor": 1.5},
            },
            "review": {"max_rounds": 3, "reviewers": ["owner"]},
        },
        "gates": {
            "tests": "unit",
            "run": [
                {"name": "unit", "command": "python3 tests/test_ok.py"},
                {"name": "drift", "command": f"python3 {RENDERER_PATH} --check"},
                {"name": "shell", "command": "true && test -d src"},
            ],
            "report": {"formats": ["ctrf", "junit"]},
        },
    }


def ready_files() -> dict:
    return {
        ".github/CODEOWNERS": b"# Code owners\n* @owner\n",
        "AGENTS.md": b"# Agents\n",
        "CLAUDE.md": b"@AGENTS.md\n",
        "CONTRIBUTING.md": b"# Contributing\n",
        "README.md": b"# Fixture\n",
        "docs/architecture/overview.md": b"# Overview\n",
        "src/main.py": b"print('main')\n",
        "tests/test_ok.py": b"print('ok')\n",
        ".mintswarm/stages.md": b"# Stage conventions\n",
        ".mintswarm/stages/probe.md": AGENT_SOURCE,
        ".claude/agents/msw-probe.md": AGENT_RENDERED,
        RENDERER_PATH: RENDERER.read_bytes(),
    }


def as_json(config: dict) -> bytes:
    return (json.dumps(config, indent=2) + "\n").encode("utf-8")


def set_gates(config: dict, gates: list) -> None:
    config["gates"]["run"] = gates


# Gate scripts for --run-gates, so the gate commands stay inside the reader's grammar (no `$`).
GATE_SCRIPTS = {
    "tests/slow.py": (
        "import os, time\n"
        f"with open(os.environ[{PIDFILE!r}], 'w') as out:\n"
        "    out.write('%d\\n' % os.getpgid(0))\n"
        "time.sleep(30)\n"
    ),
    "tests/background.py": (
        "import os, subprocess\n"
        "child = subprocess.Popen(['sleep', '40'], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,"
        " stderr=subprocess.DEVNULL)\n"
        f"with open(os.environ[{BACKGROUND!r}], 'w') as out:\n"
        "    out.write('%d\\n' % child.pid)\n"
    ),
    "tests/environment.py": f"import os\nopen(os.environ[{GATE_OUT!r}], 'w').write('ran\\n')\n",
    "tests/where.py": (
        "import os, subprocess\n"
        "head = subprocess.run(['git', 'rev-parse', 'HEAD'], capture_output=True, text=True).stdout\n"
        "status = subprocess.run(['git', 'status', '--porcelain'], capture_output=True, text=True).stdout\n"
        f"open(os.environ[{GATE_OUT!r}], 'w').write(head + status)\n"
    ),
}


def with_gates(gates: list):
    """A mutation that sets gates.run and adds the gate scripts they name."""

    def mutate(config, files):
        set_gates(config, gates)
        for path, source in GATE_SCRIPTS.items():
            if any(path in gate["command"] for gate in gates):
                files[path] = source.encode("utf-8")

    return mutate


class Sandbox:
    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.repo = tmp / "repo"
        self.checktmp = tmp / "checktmp"
        self.out = tmp / "out"
        home = tmp / "home"
        for directory in (self.repo, self.checktmp, self.out, home):
            directory.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(
            {
                "HOME": str(home),
                "TMPDIR": str(self.checktmp),
                "PYTHONDONTWRITEBYTECODE": "1",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_AUTHOR_NAME": "readiness selftest",
                "GIT_AUTHOR_EMAIL": "selftest@example.invalid",
                "GIT_COMMITTER_NAME": "readiness selftest",
                "GIT_COMMITTER_EMAIL": "selftest@example.invalid",
                "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
                "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
                PIDFILE: str(tmp / "gate.pid"),
                GATE_OUT: str(tmp / "gate-ran"),
                BACKGROUND: str(tmp / "background.pid"),
            }
        )
        self.env = env
        self.counter = 0
        self.git("init", "-q", "-b", "master")

    def raw(self, *args: str, data: bytes = None, env: dict = None) -> bytes:
        proc = subprocess.run(["git", *args], cwd=self.repo, env=env or self.env, input=data, capture_output=True)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.decode(errors='replace').strip()}")
        return proc.stdout

    def git(self, *args: str, data: bytes = None) -> str:
        return self.raw(*args, data=data).decode("utf-8").strip()

    def commit(self, files: dict) -> str:
        """Commit files ({path: bytes or (mode, bytes)}) as the whole tree."""
        self.counter += 1
        records = b""
        for path, content in sorted(files.items()):
            mode, data = content if isinstance(content, tuple) else ("100644", content)
            sha = self.git("hash-object", "-w", "--stdin", data=data)
            records += f"{mode} {sha}\t".encode("utf-8") + path.encode("utf-8") + b"\0"
        index = self.tmp / f"index-{self.counter}"
        env = dict(self.env, GIT_INDEX_FILE=str(index))
        self.raw("update-index", "-z", "--index-info", data=records, env=env)
        tree = self.raw("write-tree", env=env).decode("ascii").strip()
        os.unlink(index)
        return self.git("commit-tree", tree, "-m", f"fixture {self.counter}")

    def fixture(self, mutate=None) -> str:
        config, files = ready_config(), ready_files()
        if mutate is not None:
            mutate(config, files)
        if config is not None and CONFIG not in files:
            files[CONFIG] = as_json(config)
        return self.commit({path: data for path, data in files.items() if data is not None})

    def worktrees(self) -> str:
        return self.git("worktree", "list", "--porcelain")

    def run(self, args: list, env: dict = None, script: Path = CHECK, verdict: bool = True) -> tuple:
        """Run the check; return (proc, verdict bytes or None). A stale verdict is put at the --json path first."""
        self.counter += 1
        path = self.out / f"verdict-{self.counter}.json"
        if verdict:
            args = [*args, "--json", str(path)]
        targets = [Path(v) for a, v in zip(args, args[1:]) if a == "--json" and v.startswith(str(self.tmp))]
        for target in targets:
            target.write_text('{"stale": true}\n', encoding="utf-8")
        proc = subprocess.run(
            [sys.executable, "-I", str(script), *args], cwd=self.repo, env=env or self.env, capture_output=True
        )
        data = None
        for target in targets:
            if target.exists():
                data = target.read_bytes()
        return proc, data


def text(data: bytes) -> str:
    return data.decode("utf-8", errors="replace")


def parse_verdict(data):
    try:
        return json.loads(data.decode("utf-8")) if data is not None else None
    except ValueError:
        return None


def expected_lines(verdict: dict) -> list:
    lines = []
    for s in verdict["signals"]:
        line = f"{s['result']:7} {s['feature']} {s['signal']}"
        if s["reasons"]:
            line += f": {s['reasons'][0]}"
        lines.append(line)
    return lines + [f"readiness: {verdict['verdict']}"]


def shape_problems(sb: Sandbox, proc, data, ref: str, commit: str, gate_signals: list = ()) -> list:
    """What is wrong with the verdict and the printed lines of a run that exits 0 or 1."""
    problems = []
    verdict = parse_verdict(data)
    if not isinstance(verdict, dict):
        return [f"no verdict (exit {proc.returncode}, stderr {text(proc.stderr)!r})"]
    if list(verdict) != VERDICT_KEYS:
        problems.append(f"keys {list(verdict)}")
        return problems
    if verdict["tool"] != "mintswarm-readiness" or verdict["version"] != 1:
        problems.append(f"tool {verdict['tool']!r} version {verdict['version']!r}")
    if verdict["ref"] != {"ref": ref, "commit": commit}:
        problems.append(f"ref {verdict['ref']}")
    ids = [(s.get("feature"), s.get("signal")) for s in verdict["signals"]]
    if ids != SIGNALS + list(gate_signals):
        problems.append(f"signals {ids}")
    for s in verdict["signals"]:
        if list(s) != SIGNAL_KEYS or s["result"] not in ("pass", "fail") or not isinstance(s["reasons"], list):
            problems.append(f"signal {s}")
        elif s["result"] == "fail" and not s["reasons"]:
            problems.append(f"{s['signal']} fails without a reason")
    gaps = any(s.get("result") == "fail" for s in verdict["signals"])
    if verdict["verdict"] != ("gaps" if gaps else "ready") or proc.returncode != (1 if gaps else 0):
        problems.append(f"verdict {verdict['verdict']} with exit {proc.returncode}")
    if text(proc.stdout).split("\n") != expected_lines(verdict) + [""]:
        problems.append(f"printed lines {text(proc.stdout)!r}")
    for place in (str(sb.tmp), os.path.realpath(sb.tmp)):
        if place in text(proc.stdout) or place in text(data):
            problems.append("a temporary path is printed or written")
    if proc.stderr:
        problems.append(f"stderr {text(proc.stderr)!r}")
    return problems


def failing(data) -> dict:
    verdict = parse_verdict(data) or {"signals": []}
    return {s["signal"]: s for s in verdict["signals"] if s["result"] == "fail"}


def line_of(proc, name: str) -> str:
    for line in text(proc.stdout).split("\n"):
        parts = line.split(" ")
        words = [p for p in parts if p]
        if len(words) >= 3 and words[2].rstrip(":") == name:
            return line
    return ""


# --- static cases -----------------------------------------------------------


def m_set(*keys_value):
    """A mutation that sets config[k1][k2]... to value."""
    *keys, value = keys_value

    def mutate(config, files):
        target = config
        for key in keys[:-1]:
            target = target[key]
        target[keys[-1]] = value

    return mutate


def m_del(*keys):
    def mutate(config, files):
        target = config
        for key in keys[:-1]:
            target = target[key]
        del target[keys[-1]]

    return mutate


def m_file(path, data):
    def mutate(config, files):
        files[path] = data

    return mutate


def m_gate(command, name="unit"):
    def mutate(config, files):
        for gate in config["gates"]["run"]:
            if gate["name"] == name:
                gate["command"] = command

    return mutate


def m_web(*paths):
    """Track each path, with web/** in component code and in pipeline.plan_approval.require_human."""

    def mutate(config, files):
        config["components"]["code"]["paths"].append("web/**")
        config["pipeline"]["plan_approval"]["require_human"]["paths"].append("web/**")
        for path in paths:
            files[path] = b"x\n"

    return mutate


def m_web_gate(command, *paths):
    """The shell gate runs command, with web/index.ts and each path tracked."""
    return m_all(m_web("web/index.ts", *paths), m_gate(command, name="shell"))


def cd_gap(name, command, fragment, *paths):
    """A gap case for a command the reader cannot read because of how it runs cd."""
    return (
        name,
        m_web_gate(command, *paths),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": f"gate shell: {fragment}",
            "gate-tools": f"gate shell: {fragment}",
        },
        (),
    )


def script_gap(name, command, fragment, *paths, needs=()):
    """A gap case for a readable command that runs code no tracked regular file holds."""
    return (
        name,
        m_web_gate(command, *paths),
        {"gate-scripts-named"},
        {"gate-scripts-named": f"gate shell: {fragment}"},
        needs,
    )


# The repository's own TypeScript gates, verbatim from .mintswarm/config.yaml.
TYPESCRIPT_GATES = {
    "typescript-typecheck": "bash .mintswarm/dogfood/web-install.sh && cd ./web && ./node_modules/.bin/tsc --noEmit",
    "typescript-lint": "bash .mintswarm/dogfood/web-install.sh && cd ./web && ./node_modules/.bin/eslint .",
    "typescript-format": "bash .mintswarm/dogfood/web-install.sh && cd ./web && ./node_modules/.bin/prettier --check .",
    "typescript-test": "bash .mintswarm/dogfood/web-install.sh && cd ./web/apps/ui && ./node_modules/.bin/vitest run",
}


def m_typescript_gates(config, files):
    m_web("web/pnpm-lock.yaml", "web/apps/ui/index.ts")(config, files)
    files[".mintswarm/dogfood/web-install.sh"] = b"exit 0\n"
    config["gates"]["run"] += [{"name": name, "command": command} for name, command in TYPESCRIPT_GATES.items()]


def m_typescript_gates_direct(config, files):
    """The repository's TypeScript gates, running web-install.sh as the command word rather than through bash."""
    m_typescript_gates(config, files)
    for gate in config["gates"]["run"]:
        if gate["name"] in TYPESCRIPT_GATES:
            gate["command"] = gate["command"].replace("bash .mintswarm/dogfood/", "./.mintswarm/dogfood/")


def m_all(*mutations):
    def mutate(config, files):
        for mutation in mutations:
            mutation(config, files)

    return mutate


def m_no_renderer(config, files):
    files[RENDERER_PATH] = None
    config["gates"]["run"] = [g for g in config["gates"]["run"] if g["name"] != "drift"]


def m_no_tests(config, files):
    files["tests/test_ok.py"] = None
    config["gates"]["run"] = [g for g in config["gates"]["run"] if g["name"] != "unit"]


def m_overlap(config, files):
    config["components"]["code"]["paths"].append("docs/architecture/**")
    files["stray.txt"] = b"stray\n"


def m_move_architecture(config, files):
    files["docs/guide.md"] = files.pop("docs/architecture/overview.md")


def m_move_codeowners(config, files):
    files["docs/CODEOWNERS"] = files.pop(".github/CODEOWNERS")


def m_rh_plan(config, files):
    config["pipeline"]["plan_approval"]["require_human"]["paths"].remove("tests/**")


def m_rh_landing(config, files):
    config["pipeline"]["landing"]["require_human"]["paths"].remove(".claude/**")


def m_duplicate_gate(config, files):
    config["gates"]["run"].append({"name": "unit", "command": "true"})


def m_rename_docs(config, files):
    config["components"]["documentation"] = config["components"].pop("docs")


PASS_GATE = "python3 ./tests/test_ok.py --flag >/dev/null 2>&1 || bash tests/test_ok.py -x"

GAP_CASES = [
    # (name, mutation, failing signals, {signal: reason fragment})
    (
        "plan kind without issue_type",
        m_set("tracker", "kinds", "plan", {"label": "Plan"}),
        {"tracker-kinds"},
        {"tracker-kinds": "tracker.kinds.plan.issue_type is not set"},
    ),
    (
        "component label without {name}",
        m_set("tracker", "components", "label", "component"),
        {"tracker-kinds"},
        {"tracker-kinds": "has no {name}"},
    ),
    (
        "no trusted labels or authors",
        m_set("tracker", "ingest", "trusted", {"labels": [], "authors": []}),
        {"trusted-ingest"},
        {"trusted-ingest": "no labels and no authors"},
    ),
    (
        "match all with no authors",
        m_all(m_set("tracker", "ingest", "match", "all"), m_set("tracker", "ingest", "trusted", "authors", [])),
        {"trusted-ingest"},
        {"trusted-ingest": "match is all but trusted.authors is empty"},
    ),
    (
        "an uncovered file",
        m_file("stray.txt", b"stray\n"),
        {"components-cover"},
        {"components-cover": "stray.txt is in no component and not in shared"},
    ),
    (
        "an overlap next to an uncovered file",
        m_overlap,
        {"components-cover"},
        {"components-cover": "docs/architecture/overview.md is in more than one of components code, docs"},
    ),
    (
        "an overlap with shared",
        m_set("shared", [CONFIG, "README.md"]),
        {"components-cover"},
        {"components-cover": "README.md is in more than one of components docs and shared"},
    ),
    ("no CODEOWNERS", m_file(".github/CODEOWNERS", None), {"codeowners"}, {"codeowners": "no CODEOWNERS file"}),
    (
        "CODEOWNERS without an owner",
        m_file(".github/CODEOWNERS", b"# none\n*\n"),
        {"codeowners"},
        {"codeowners": ".github/CODEOWNERS has no rule naming an owner"},
    ),
    (
        "no default reviewers",
        m_set("pipeline", "review", "reviewers", []),
        {"reviewers"},
        {"reviewers": ".claude/agents/msw-probe.md has no reviewer"},
    ),
    (
        "an invalid login",
        m_set("components", "code", "reviewers", ["@co der"]),
        {"reviewers"},
        {"reviewers": "is not a valid login"},
    ),
    ("no AGENTS.md", m_file("AGENTS.md", None), {"agents-md"}, {"agents-md": "AGENTS.md is not a regular file"}),
    (
        "CLAUDE.md without the pointer",
        m_file("CLAUDE.md", b"See AGENTS.md\n"),
        {"claude-md"},
        {"claude-md": "does not read AGENTS.md"},
    ),
    (
        "CLAUDE.md linking elsewhere",
        m_file("CLAUDE.md", (SYMLINK, b"README.md")),
        {"claude-md"},
        {"claude-md": "neither a symbolic link to AGENTS.md"},
    ),
    (
        "no CONTRIBUTING.md",
        m_file("CONTRIBUTING.md", None),
        {"contributing"},
        {"contributing": "CONTRIBUTING.md is not a regular file"},
    ),
    (
        "no docs/architecture/",
        m_move_architecture,
        {"architecture-docs"},
        {"architecture-docs": "nothing is tracked under docs/architecture/"},
    ),
    (
        "a rendered agent edited by hand",
        m_file(".claude/agents/msw-probe.md", AGENT_RENDERED + b"edited\n"),
        {"agents-drift"},
        {"agents-drift": "differs: .claude/agents/msw-probe.md"},
    ),
    (
        "an orphan rendered agent",
        m_file(".claude/agents/msw-ghost.md", b"ghost\n"),
        {"agents-drift"},
        {"agents-drift": "orphan: .claude/agents/msw-ghost.md"},
    ),
    (
        "a missing rendered agent",
        m_file(".claude/agents/msw-probe.md", None),
        {"agents-drift"},
        {"agents-drift": "missing: .claude/agents/msw-probe.md"},
    ),
    (
        "a symlinked rendered agent",
        m_file(".claude/agents/msw-probe.md", (SYMLINK, b"msw-other.md")),
        {"agents-drift"},
        {"agents-drift": ".claude/agents/msw-probe.md is not a regular file"},
    ),
    (
        "a broken stage source",
        m_file(".mintswarm/stages/probe.md", b"no frontmatter\n"),
        {"agents-drift"},
        {"agents-drift": ".mintswarm/stages/probe.md: must start with a '---' line"},
    ),
    ("no gates", m_set("gates", "run", []), {"gates-run", "tests-gated"}, {"gates-run": "gates.run is empty"}),
    ("a gate name used twice", m_duplicate_gate, {"gates-run"}, {"gates-run": "the name unit is used twice"}),
    (
        "bash -c",
        m_gate("bash -c true; python3 tests/test_ok.py"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate unit: bash is given -c before its script, an option the check does not read",
            "gate-tools": "gate unit: bash is given -c before its script, an option the check does not read, so its tools are unknown",
        },
    ),
    (
        "python3 -m",
        m_gate("python3 -m unittest tests/test_ok.py"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate unit: python3 is given -m before its script, an option the check does not read",
            "gate-tools": "gate unit: python3 is given -m before its script, an option the check does not read, so its tools are unknown",
        },
    ),
    (
        "an untracked script path",
        m_gate("./scripts/missing.sh tests/test_ok.py"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate unit: it runs ./scripts/missing.sh, which is not a tracked file"},
    ),
    (
        "an untracked interpreter script",
        m_gate("python3 tests/test_ok.py && python3 tests/gone.py"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "python3 runs tests/gone.py, which is not a tracked file"},
    ),
    (
        "a command substitution",
        m_gate("python3 tests/test_ok.py $(echo -q)"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate unit: it holds '$', which the check does not read",
            "gate-tools": "gate unit: it holds '$', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "eval",
        m_gate("eval python3 tests/test_ok.py"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate unit: it runs eval, a builtin the check does not read",
            "gate-tools": "gate unit: it runs eval, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "an expanded command word",
        m_gate("$PYTHON tests/test_ok.py"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate unit: it holds '$', which the check does not read",
            "gate-tools": "gate unit: it holds '$', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a command that does not split",
        m_gate("python3 'tests/test_ok.py"),
        ["gate-scripts-named", "gate-tools", "plan-approval-config", "tests-gated"],
        {
            "gate-scripts-named": 'gate unit: it holds "\'", which the check does not read',
            "gate-tools": "its tools are unknown",
            "plan-approval-config": "command does not split",
        },
    ),
    (
        "a process substitution",
        m_gate("python3 tests/test_ok.py <(true)"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate unit: it holds '<', which the check does not read",
            "gate-tools": "gate unit: it holds '<', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a # inside a word",
        m_gate("echo a#b; ./untracked.sh", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '#', which the check does not read",
            "gate-tools": "gate shell: it holds '#', which the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "$#",
        m_gate("echo $#; bash -c x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '$', which the check does not read",
            "gate-tools": "gate shell: it holds '$', which the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "env -u NAME",
        m_gate("env -u X bash -c evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs env, a prefix the check does not read",
            "gate-tools": "gate shell: it runs env, a prefix the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "env -S",
        m_gate("env -S bash", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs env, a prefix the check does not read",
            "gate-tools": "gate shell: it runs env, a prefix the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "exec -a NAME",
        m_gate("exec -a name bash -c evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs exec, a prefix the check does not read",
            "gate-tools": "gate shell: it runs exec, a prefix the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "time -p",
        m_gate("time -p bash -c evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs time, a prefix the check does not read",
            "gate-tools": "gate shell: it runs time, a prefix the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "time with an unknown option",
        m_gate("time -x true", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs time, a prefix the check does not read",
            "gate-tools": "gate shell: it runs time, a prefix the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "command",
        m_gate("command bash -c evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs command, a prefix the check does not read",
            "gate-tools": "gate shell: it runs command, a prefix the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "builtin source",
        m_gate("builtin source /tmp/evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs builtin, a builtin the check does not read",
            "gate-tools": "gate shell: it runs builtin, a builtin the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        ". of an untracked file",
        m_gate(". ./untracked.sh", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: . runs ./untracked.sh, which is not a tracked file"},
        (),
    ),
    (
        "coproc",
        m_gate("coproc bash -c evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs coproc, a builtin the check does not read",
            "gate-tools": "gate shell: it runs coproc, a builtin the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "trap with an action",
        m_gate("trap evil EXIT", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs trap, a builtin the check does not read",
            "gate-tools": "gate shell: it runs trap, a builtin the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "cd to an absolute path",
        m_gate("cd /tmp && true", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs cd /tmp, which does not start with ./, so CDPATH can change",
            "gate-tools": "gate shell: it runs cd /tmp, which does not start with ./, so CDPATH can change where it "
            "goes, so its tools are unknown",
        },
        (),
    ),
    cd_gap("bare cd web", "cd web && true", "it runs cd web, which does not start with ./, so CDPATH can change"),
    cd_gap("cd before ||", "cd ./web || true", "it runs cd in a command not joined by && alone"),
    cd_gap("cd before ;", "cd ./web; true", "it runs cd in a command not joined by && alone"),
    cd_gap("cd with && in a word", "cd ./web&&true", "it runs cd in a command not joined by && alone"),
    cd_gap("cd on two lines", "cd ./web &&\ntrue", "it runs cd in a command not joined by && alone"),
    cd_gap("cd inside if", "if true; then cd ./web; fi", "it runs cd in a command not joined by && alone"),
    cd_gap("cd after while", "while cd ./web && true", "it runs cd in a command with while"),
    cd_gap("cd through ..", "cd ./web/../src && true", "it runs cd ./web/../src, which has an empty or .. segment"),
    cd_gap("cd with an empty segment", "cd ./web/ && true", "it runs cd ./web/, which has an empty or .. segment"),
    cd_gap("cd to a missing directory", "cd ./missing && true", "it runs cd ./missing, which is not a tracked"),
    cd_gap(
        "cd to a missing directory below another",
        "cd ./web && cd ./missing && true",
        "it runs cd ./missing (as web/missing), which is not a tracked directory",
    ),
    (
        "cd into a tracked symbolic link",
        m_all(m_web_gate("cd ./web/link && true"), m_file("web/link", (SYMLINK, b"."))),
        ["gate-scripts-named", "gate-tools"],
        {"gate-scripts-named": "gate shell: it runs cd ./web/link, which is not a tracked directory"},
        (),
    ),
    (
        "node_modules/.bin as a tracked symbolic link",
        m_all(
            m_web_gate("cd ./web && ./node_modules/.bin/tsc", "web/pnpm-lock.yaml"),
            m_file("web/node_modules/.bin/tsc", (SYMLINK, b"../../index.ts")),
        ),
        {"gate-scripts-named"},
        {"gate-scripts-named": "it runs web/node_modules/.bin/tsc, which is a tracked symbolic link"},
        (),
    ),
    (
        "node_modules as a tracked symbolic link",
        m_all(
            m_web_gate("cd ./web && ./node_modules/.bin/tsc", "web/pnpm-lock.yaml", "web/evil/.bin/tsc"),
            m_file("web/node_modules", (SYMLINK, b"evil")),
        ),
        {"gate-scripts-named"},
        {
            "gate-scripts-named": "it runs web/node_modules/.bin/tsc, which is under the tracked symbolic link "
            "web/node_modules"
        },
        (),
    ),
    (
        "node_modules/.bin under a tracked symbolic link",
        m_all(
            m_web_gate("cd ./web && ./node_modules/.bin/tsc", "web/pnpm-lock.yaml", "web/evil/tsc"),
            m_file("web/node_modules/.bin", (SYMLINK, b"../evil")),
        ),
        {"gate-scripts-named"},
        {
            "gate-scripts-named": "it runs web/node_modules/.bin/tsc, which is under the tracked symbolic link "
            "web/node_modules/.bin"
        },
        (),
    ),
    (
        "node_modules/.bin under a symbolically linked directory",
        m_all(
            m_web_gate("cd ./web && sub/node_modules/.bin/tsc", "web/pnpm-lock.yaml", "web/evil/node_modules/.bin/tsc"),
            m_file("web/sub", (SYMLINK, b"evil")),
        ),
        {"gate-scripts-named"},
        {
            "gate-scripts-named": "it runs web/sub/node_modules/.bin/tsc, which is under the tracked symbolic link "
            "web/sub"
        },
        (),
    ),
    script_gap(
        "node_modules/.bin under an untracked directory",
        "cd ./web && nothere/node_modules/.bin/tsc",
        "it runs web/nothere/node_modules/.bin/tsc, whose directory web/nothere is not a tracked directory",
        "web/pnpm-lock.yaml",
    ),
    cd_gap("cd with an option", "cd -P ./web && true", "it runs cd with 2 arguments, but the check reads cd"),
    cd_gap("cd with no argument", "cd && true", "it runs cd with 0 arguments, but the check reads cd"),
    cd_gap("cd with a redirection", "cd >/dev/null ./web && true", "it runs cd with a redirection"),
    script_gap(
        "an untracked script after cd",
        "cd ./web && bash x.sh",
        "bash runs web/x.sh, which is not a tracked file",
    ),
    script_gap(
        "node_modules/.bin with no lockfile",
        "./node_modules/.bin/tsc --noEmit",
        "it runs ./node_modules/.bin/tsc, which is not a tracked file",
    ),
    script_gap(
        "node_modules/.bin with a lockfile in another directory",
        "cd ./web && ./node_modules/.bin/tsc --noEmit",
        "it runs web/node_modules/.bin/tsc, which is not a tracked file",
        "src/pnpm-lock.yaml",
    ),
    script_gap(
        "node_modules/.bin climbing out",
        "cd ./web && ./node_modules/.bin/../../evil",
        "it runs web/node_modules/.bin/../../evil, which is not a tracked file",
        "web/pnpm-lock.yaml",
    ),
    script_gap(
        "nested node_modules",
        "cd ./web && a/node_modules/b/node_modules/.bin/x",
        "it runs web/a/node_modules/b/node_modules/.bin/x, which is not a tracked file",
        "web/pnpm-lock.yaml",
    ),
    script_gap(
        "node_modules/.bin under a yarn lockfile",
        "cd ./web && ./node_modules/.bin/tsc",
        "it runs web/node_modules/.bin/tsc, which is not a tracked file",
        "web/yarn.lock",
    ),
    script_gap(
        "pnpm install",
        "pnpm --dir web install --frozen-lockfile",
        "it runs pnpm, which runs what an untracked recipe says",
        "web/pnpm-lock.yaml",
        needs=("pnpm",),
    ),
    (
        "timeout",
        m_gate("timeout 600 ./scripts/missing.sh", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: it runs timeout, which runs another command"},
        ("timeout",),
    ),
    (
        "nice",
        m_gate("nice bash -c x", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: it runs nice, which runs another command"},
        ("nice",),
    ),
    (
        "xargs",
        m_gate("xargs sh -c x", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: it runs xargs, which runs another command"},
        ("xargs",),
    ),
    (
        "sudo",
        m_gate("sudo bash -c x", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: it runs sudo, which runs another command"},
        ("sudo",),
    ),
    (
        "find -exec",
        m_gate("find . -exec sh -c x +", name="shell"),
        ["gate-scripts-named"],
        {"gate-scripts-named": "gate shell: it runs find, which runs another command"},
        ("find",),
    ),
    (
        "a versioned interpreter",
        m_gate("python3.12 -c x", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: it runs python3.12, an interpreter the check does not read"},
        ("python3.12",),
    ),
    (
        "an unmodelled interpreter",
        m_gate("awk -f tests/test_ok.py", name="shell"),
        ["gate-scripts-named"],
        {"gate-scripts-named": "gate shell: it runs awk, an interpreter the check does not read"},
        ("awk",),
    ),
    (
        "brace expansion",
        m_gate("{bash,-c,evil}", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '{', which the check does not read",
            "gate-tools": "gate shell: it holds '{', which the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "node -r",
        m_gate("node -r ./evil.js tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: node is given -r before its script, an option the check does not read",
            "gate-tools": "gate shell: node is given -r before its script, an option the check does not read, so its tools are unknown",
        },
        ("node",),
    ),
    (
        "ruby -r",
        m_gate("ruby -rmodule tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: ruby is given -rmodule before its script, an option the check does not read",
            "gate-tools": "gate shell: ruby is given -rmodule before its script, an option the check does not read, so its tools are unknown",
        },
        ("ruby",),
    ),
    (
        "perl -M",
        m_gate("perl -MEvil tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: perl is given -MEvil before its script, an option the check does not read",
            "gate-tools": "gate shell: perl is given -MEvil before its script, an option the check does not read, so its tools are unknown",
        },
        ("perl",),
    ),
    (
        "node --require",
        m_gate("node --require=./evil.js tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: node is given --require=./evil.js before its script, an option the check does not read",
            "gate-tools": "gate shell: node is given --require=./evil.js before its script, an option the check does not read, so its tools are unknown",
        },
        ("node",),
    ),
    (
        "an option the reader does not know",
        m_gate("python3 -j tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: python3 is given -j before its script, an option the check does not read",
            "gate-tools": "gate shell: python3 is given -j before its script, an option the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "BASH_ENV",
        m_gate("BASH_ENV=evil.sh bash tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: its command word BASH_ENV=evil.sh holds =, which the check does not read",
            "gate-tools": "gate shell: its command word BASH_ENV=evil.sh holds =, which the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "PATH",
        m_gate("PATH=. python3 tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: its command word PATH=. holds =, which the check does not read",
            "gate-tools": "gate shell: its command word PATH=. holds =, which the check does not read, so its tools are unknown",
        },
        (),
    ),
    (
        "npm test",
        m_gate("npm test", name="shell"),
        {"gate-scripts-named"},
        {"gate-scripts-named": "gate shell: it runs npm, which runs what an untracked recipe says"},
        ("npm",),
    ),
    (
        "command with -v after its command word",
        m_gate("command bash -c x -v", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs command, a prefix the check does not read",
            "gate-tools": "gate shell: it runs command, a prefix the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin export",
        m_gate("export BASH_ENV=./untracked.sh; bash tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs export, a builtin the check does not read",
            "gate-tools": "gate shell: it runs export, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin declare",
        m_gate("declare -x BASH_ENV=./untracked.sh", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs declare, a builtin the check does not read",
            "gate-tools": "gate shell: it runs declare, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin typeset",
        m_gate("typeset -x BASH_ENV=./untracked.sh", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs typeset, a builtin the check does not read",
            "gate-tools": "gate shell: it runs typeset, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin local",
        m_gate("local BASH_ENV=./untracked.sh", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs local, a builtin the check does not read",
            "gate-tools": "gate shell: it runs local, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin readonly",
        m_gate("readonly NODE_OPTIONS=--require=./evil.js", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs readonly, a builtin the check does not read",
            "gate-tools": "gate shell: it runs readonly, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin read",
        m_gate("read BASH_ENV", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs read, a builtin the check does not read",
            "gate-tools": "gate shell: it runs read, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin unset",
        m_gate("unset -n BASH_ENV", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs unset, a builtin the check does not read",
            "gate-tools": "gate shell: it runs unset, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin mapfile",
        m_gate("mapfile -C evil x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs mapfile, a builtin the check does not read",
            "gate-tools": "gate shell: it runs mapfile, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin let",
        m_gate("let x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs let, a builtin the check does not read",
            "gate-tools": "gate shell: it runs let, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin [[",
        m_gate("[[ x -eq 0 ]]", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '[', which the check does not read",
            "gate-tools": "gate shell: it holds '[', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin compgen",
        m_gate("compgen -C evil x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs compgen, a builtin the check does not read",
            "gate-tools": "gate shell: it runs compgen, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin complete",
        m_gate("complete -C evil x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs complete, a builtin the check does not read",
            "gate-tools": "gate shell: it runs complete, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "the builtin bind",
        m_gate("bind -x evil", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs bind, a builtin the check does not read",
            "gate-tools": "gate shell: it runs bind, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "printf -v",
        m_gate("printf -v BASH_ENV %s ./untracked.sh", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs printf, a builtin the check does not read",
            "gate-tools": "gate shell: it runs printf, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "set -x",
        m_gate("set -x; true", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs set, a builtin the check does not read",
            "gate-tools": "gate shell: it runs set, a builtin the check does not read, so its tools are unknown",
        },
    ),
    (
        "a PS4 expansion",
        m_gate("PS4='$''(touch pwn)'; set -x; true", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": 'gate shell: it holds "\'", which the check does not read',
            "gate-tools": 'gate shell: it holds "\'", which the check does not read, so its tools are unknown',
        },
    ),
    (
        "((",
        m_gate("(( ls ))", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '(', which the check does not read",
            "gate-tools": "gate shell: it holds '(', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "an assignment through env",
        m_gate("env A=1 python3 tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs env, a prefix the check does not read",
            "gate-tools": "gate shell: it runs env, a prefix the check does not read, so its tools are unknown",
        },
    ),
    (
        "a prefix assignment",
        m_gate("A=1 true", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: its command word A=1 holds =, which the check does not read",
            "gate-tools": "gate shell: its command word A=1 holds =, which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a symlinked script",
        m_all(
            m_file("tests/run.sh", (SYMLINK, b"../outside.sh")),
            m_gate("bash tests/run.sh; ./tests/run.sh", name="shell"),
        ),
        ["gate-scripts-named"],
        {
            "gate-scripts-named": "gate shell: bash runs tests/run.sh, which is a tracked symbolic link, not a regular file"
        },
    ),
    (
        "env -u with a brace expansion",
        m_gate("env -u {X,bash,-c,x}; python3 tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '{', which the check does not read",
            "gate-tools": "gate shell: it holds '{', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "exec -a with a brace expansion",
        m_gate("exec -a {x,bash,-c,x}", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '{', which the check does not read",
            "gate-tools": "gate shell: it holds '{', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "python3 -W with a brace expansion",
        m_gate("python3 -W {ignore,-c,x} tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '{', which the check does not read",
            "gate-tools": "gate shell: it holds '{', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "bash -o with a brace expansion",
        m_gate("bash -o {posix,-c,x} tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '{', which the check does not read",
            "gate-tools": "gate shell: it holds '{', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a digit word apart from >",
        m_gate("echo touch pwn > 2; bash 2 > out tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '>', which the check does not read",
            "gate-tools": "gate shell: it holds '>', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a digit word before a redirection",
        m_gate("bash 2 >/dev/null tests/test_ok.py", name="shell"),
        ["gate-scripts-named"],
        {"gate-scripts-named": "gate shell: bash runs 2, which is not a tracked file"},
    ),
    (
        "python3 with a digit word apart from >",
        m_gate("python3 1 > x tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '>', which the check does not read",
            "gate-tools": "gate shell: it holds '>', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "python3 with a digit word before a redirection",
        m_gate("python3 1 >/dev/null tests/test_ok.py", name="shell"),
        ["gate-scripts-named"],
        {"gate-scripts-named": "gate shell: python3 runs 1, which is not a tracked file"},
    ),
    (
        "a quoted operator",
        m_gate("echo touch pwn | tee '>' x; bash '>' x tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": 'gate shell: it holds "\'", which the check does not read',
            "gate-tools": 'gate shell: it holds "\'", which the check does not read, so its tools are unknown',
        },
    ),
    (
        "a redirection to a file",
        m_gate("python3 tests/test_ok.py 2>x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '2>x', which the check does not read",
            "gate-tools": "gate shell: it holds '2>x', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a background &",
        m_gate("true & python3 tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it holds '&', which the check does not read",
            "gate-tools": "gate shell: it holds '&', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a trailing operator",
        m_gate("true &&", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it ends with '&&', which the check does not read",
            "gate-tools": "gate shell: it ends with '&&', which the check does not read, so its tools are unknown",
        },
    ),
    (
        "NAME+= before a command",
        m_gate("BASH_ENV+=tests/p bash tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: its command word BASH_ENV+=tests/p holds =, which the check does not read",
            "gate-tools": "gate shell: its command word BASH_ENV+=tests/p holds =, which the check does not read, so its tools are unknown",
        },
    ),
    (
        "env with any word holding =",
        m_gate("env 9=x/ok bash pwn", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs env, a prefix the check does not read",
            "gate-tools": "gate shell: it runs env, a prefix the check does not read, so its tools are unknown",
        },
    ),
    (
        "python3 -i",
        m_gate("echo x | python3 -i tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: python3 is given -i before its script, an option the check does not read",
            "gate-tools": "gate shell: python3 is given -i before its script, an option the check does not read, so its tools are unknown",
        },
    ),
    (
        "zsh -O",
        m_gate("zsh -O x tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: zsh is given -O before its script, an option the check does not read",
            "gate-tools": "gate shell: zsh is given -O before its script, an option the check does not read, so its tools are unknown",
        },
    ),
    (
        "bash +x",
        m_gate("bash +x x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: bash is given +x before its script, an option the check does not read",
            "gate-tools": "gate shell: bash is given +x before its script, an option the check does not read, so its tools are unknown",
        },
    ),
    (
        "source with an option",
        m_gate("source -x tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: source is given -x before its script, an option the check does not read",
            "gate-tools": "gate shell: source is given -x before its script, an option the check does not read, so its tools are unknown",
        },
    ),
    (
        "time -p --",
        m_gate("time -p -- bash x", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: it runs time, a prefix the check does not read",
            "gate-tools": "gate shell: it runs time, a prefix the check does not read, so its tools are unknown",
        },
    ),
    (
        "a command word starting with +",
        m_gate("+x tests/test_ok.py", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: its command word +x starts with +, which the check does not read",
            "gate-tools": "gate shell: its command word +x starts with +, which the check does not read, so its tools are unknown",
        },
    ),
    (
        "an interpreter given no script",
        m_gate("echo x | python3", name="shell"),
        ["gate-scripts-named", "gate-tools"],
        {
            "gate-scripts-named": "gate shell: python3 is given no script, which the check does not read",
            "gate-tools": "gate shell: python3 is given no script, which the check does not read, so its tools are unknown",
        },
    ),
    (
        "a missing tool",
        m_gate("msw-no-such-tool --flag", name="shell"),
        {"gate-tools"},
        {"gate-tools": "msw-no-such-tool is not on PATH, so the gates that call it cannot run here"},
    ),
    (
        "a test no gate runs",
        m_file("tests/test_more.py", b"print('more')\n"),
        {"tests-gated"},
        {"tests-gated": "test tests/test_more.py is named by no gate command"},
    ),
    (
        "a format without a converter",
        m_set("gates", "report", "formats", ["ctrf", "sarif"]),
        {"report-formats"},
        {"report-formats": "no converter for sarif"},
    ),
    (
        "plan approval limits without lines",
        m_del("pipeline", "plan_approval", "limits", "lines"),
        {"plan-approval-config"},
        {"plan-approval-config": "limits must have exactly"},
    ),
    (
        "plan approval limits for an undefined component",
        m_set("pipeline", "plan_approval", "limits_by_component", {"nowhere": {"lines": 600}}),
        {"plan-approval-config"},
        {"plan-approval-config": "limits_by_component.nowhere names an undefined component"},
    ),
    (
        "plan approval limits with an unknown key",
        m_set("pipeline", "plan_approval", "limits_by_component", {"code": {"size": 600}}),
        {"plan-approval-config"},
        {"plan-approval-config": "unknown keys in pipeline.plan_approval.limits_by_component.code: size"},
    ),
    ("no docs component", m_rename_docs, {"docs-component"}, {"docs-component": "components.docs has no paths"}),
    ("no tests", m_no_tests, {"tests-exist"}, {"tests-exist": "no test file is tracked"}),
    (
        "gates.tests none",
        m_set("gates", "tests", "none"),
        {"tests-declared"},
        {
            "tests-declared": "gates.tests: none while 1 test file exists, so every plan outside the docs component "
            "goes to a human"
        },
    ),
    (
        "a gate script outside plan require_human",
        m_rh_plan,
        {"require-human-plan"},
        {"require-human-plan": "tests/test_ok.py is not covered by pipeline.plan_approval.require_human"},
    ),
    (
        "assess on the plan model",
        m_set("agents", "stages", "assess", "model", "opus"),
        {"assess-model"},
        {"assess-model": "both run on opus"},
    ),
    (
        "no plan model",
        m_del("agents", "default", "model"),
        {"assess-model"},
        {"assess-model": "no model is set for the plan stage"},
    ),
    (
        "landing without a footprint",
        m_del("pipeline", "landing", "footprint"),
        {"landing-config"},
        {"landing-config": "lines_factor"},
    ),
    (
        "landing min_confidence below 90",
        m_set("pipeline", "landing", "min_confidence", 89),
        {"landing-config"},
        {"landing-config": "min_confidence"},
    ),
    (
        "an agent outside landing require_human",
        m_rh_landing,
        {"require-human-landing"},
        {"require-human-landing": ".claude/agents/msw-probe.md is not covered by pipeline.landing.require_human"},
    ),
]

READY_CASES = [
    # (name, mutation, {signal: reason fragment of a pass})
    ("CODEOWNERS under docs/", m_move_codeowners, {}),
    ("CLAUDE.md as a symlink to AGENTS.md", m_file("CLAUDE.md", (SYMLINK, b"AGENTS.md")), {}),
    ("a gate with redirections and arguments after the script", m_gate(PASS_GATE), {}),
    (
        "the allowed builtins and source",
        m_gate("true; : x; echo -n ok; test -d src; pwd; wait; if false; then exit 1; fi; . tests/test_ok.py", "shell"),
        {},
    ),
    (
        "the repository's own gates",
        m_gate("git diff --check origin/master...HEAD; python3 .mintswarm/dogfood/render-agents.py --check", "shell"),
        {},
    ),
    ("cd into a tracked directory", m_web_gate("cd ./web && bash x.sh", "web/x.sh"), {}),
    ("cd ./. and a nested cd", m_web_gate("cd ./. && cd ./web && cd ./. && bash x.sh", "web/x.sh"), {}),
    (
        "node_modules/.bin under a pnpm lockfile",
        m_web_gate("cd ./web && ./node_modules/.bin/tsc --noEmit", "web/pnpm-lock.yaml"),
        {},
    ),
    (
        "node_modules/.bin under an npm lockfile above",
        m_web_gate("cd ./web/apps/ui && ./node_modules/.bin/vitest run", "web/package-lock.json", "web/apps/ui/a.ts"),
        {},
    ),
    ("a test named after cd", m_web_gate("cd ./web && python3 tests/test_x.py", "web/tests/test_x.py"), {}),
    (
        "partial per-component limits and non_production globs",
        m_all(
            m_set("pipeline", "plan_approval", "limits_by_component", {"code": {"lines": 600}}),
            m_set("pipeline", "plan_approval", "non_production", ["**/tests/**", "**/fixtures/**"]),
        ),
        {},
    ),
    ("a ref without render-agents.py", m_no_renderer, {"agents-drift": "not applicable: the ref has no"}),
    (
        "a ref whose renderer differs",
        m_file(RENDERER_PATH, RENDERER.read_bytes() + b"# changed\n"),
        {"agents-drift": "differs from the checker's"},
    ),
]


def check_ready(sb: Sandbox) -> tuple:
    commit = sb.fixture()
    sb.git("update-ref", "refs/heads/ready", commit)
    proc, data = sb.run(["--ref", "ready"])
    problems = shape_problems(sb, proc, data, "ready", commit)
    verdict = parse_verdict(data) or {}
    if verdict.get("config") != CONFIG or verdict.get("verdict") != "ready" or proc.returncode != 0:
        problems.append(f"config {verdict.get('config')!r}, verdict {verdict.get('verdict')!r}, exit {proc.returncode}")
    fails = sorted(failing(data))
    T.check(
        "ready: every signal passes, exit 0, verdict and lines in contract order",
        not problems and not fails,
        f"{problems} failing {fails}: {text(proc.stdout)}",
    )
    return commit, proc, data


def check_gaps(sb: Sandbox) -> None:
    for name, mutation, expected, fragments, *needs in GAP_CASES:
        if name == "a missing tool" and shutil.which("msw-no-such-tool"):
            continue
        if any(shutil.which(tool) is None for tool in (needs[0] if needs else ())):
            expected = set(expected) | {"gate-tools"}  # the wrapper or interpreter is not installed here
        commit = sb.fixture(mutation)
        proc, data = sb.run(["--ref", commit])
        problems = shape_problems(sb, proc, data, commit, commit)
        found = failing(data)
        if set(found) != set(expected):
            problems.append(f"failing {sorted(found)}, expected {sorted(expected)}")
        for signal_name, fragment in fragments.items():
            line = line_of(proc, signal_name)
            if not line.startswith(f"fail    {FEATURE[signal_name]} {signal_name}: ") or fragment not in line:
                problems.append(f"line {line!r} lacks {fragment!r}")
        T.check(f"gaps: {name}: exit 1, fails {', '.join(sorted(expected))}", not problems, "; ".join(problems))
    commit = sb.fixture(m_overlap)
    _, data = sb.run(["--ref", commit])
    reasons = failing(data).get("components-cover", {}).get("reasons", [])
    T.check(
        "gaps: an overlap and an uncovered file are named in tree order",
        len(reasons) == 2
        and reasons[0].startswith("docs/architecture/overview.md is in more than one of components code, docs")
        and reasons[1].startswith("stray.txt is in no component"),
        f"{reasons}",
    )
    # make is the one task runner commonly installed; whether gate-tools fails too depends on PATH
    commit = sb.fixture(m_gate("make check tests/test_ok.py"))
    proc, data = sb.run(["--ref", commit])
    expected = {"gate-scripts-named"} | (set() if shutil.which("make") else {"gate-tools"})
    T.check(
        "gaps: make: exit 1, fails gate-scripts-named",
        set(failing(data)) == expected
        and "it runs make, which runs what an untracked recipe says" in line_of(proc, "gate-scripts-named"),
        text(proc.stdout),
    )


def check_ready_variants(sb: Sandbox) -> None:
    for name, mutation, fragments in READY_CASES:
        commit = sb.fixture(mutation)
        proc, data = sb.run(["--ref", commit])
        problems = shape_problems(sb, proc, data, commit, commit)
        if proc.returncode != 0 or failing(data):
            problems.append(f"exit {proc.returncode}, failing {sorted(failing(data))}")
        for signal_name, fragment in fragments.items():
            line = line_of(proc, signal_name)
            if not line.startswith(f"pass    {FEATURE[signal_name]} {signal_name}: ") or fragment not in line:
                problems.append(f"line {line!r} lacks {fragment!r}")
        T.check(f"ready: {name}: exit 0", not problems, "; ".join(problems))


def check_script_tools(sb: Sandbox) -> None:
    """The repository's TypeScript gates are ready with pnpm on PATH and fail gate-tools without it, though no gate
    names pnpm: web-install.sh, which they run through bash or as the command word, needs it."""
    for how, mutation in (("", m_typescript_gates), (" run as the command word", m_typescript_gates_direct)):
        check_script_tools_case(sb, how, mutation)


def check_script_tools_case(sb: Sandbox, how: str, mutation) -> None:
    commit = sb.fixture(mutation)
    for with_pnpm in (True, False):
        bin_dir = sb.tmp / (("bin-with-pnpm" if with_pnpm else "bin-without-pnpm") + how.replace(" ", "-"))
        bin_dir.mkdir()
        for tool in ("git", "yq", "bash"):
            os.symlink(shutil.which(tool), bin_dir / tool)
        os.symlink(shutil.which("python3") or sys.executable, bin_dir / "python3")
        if with_pnpm:
            (bin_dir / "pnpm").write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            (bin_dir / "pnpm").chmod(0o755)
        proc, data = sb.run(["--ref", commit], env=dict(sb.env, PATH=str(bin_dir)))
        problems = shape_problems(sb, proc, data, commit, commit)
        found = failing(data)
        if with_pnpm:
            if proc.returncode != 0 or found:
                problems.append(f"exit {proc.returncode}, failing {sorted(found)}")
            T.check(
                f"ready: the repository's TypeScript gates{how} with pnpm on PATH: exit 0",
                not problems,
                "; ".join(problems),
            )
            continue
        if set(found) != {"gate-tools"}:
            problems.append(f"failing {sorted(found)}, expected ['gate-tools']")
        fragment = "pnpm is not on PATH, so the gates that call it cannot run here"
        if fragment not in line_of(proc, "gate-tools"):
            problems.append(f"line {line_of(proc, 'gate-tools')!r} lacks {fragment!r}")
        T.check(
            f"gaps: the repository's TypeScript gates{how} without pnpm on PATH: exit 1, fails gate-tools",
            not problems,
            "; ".join(problems),
        )


def check_no_config(sb: Sandbox) -> str:
    commit = sb.commit({path: data for path, data in ready_files().items()})
    proc, data = sb.run(["--ref", commit])
    problems = shape_problems(sb, proc, data, commit, commit)
    verdict = parse_verdict(data) or {}
    if verdict.get("config", "absent") is not None:
        problems.append(f"config {verdict.get('config')!r}")
    found = failing(data)
    if set(found) != CONFIG_SIGNALS:
        problems.append(f"failing {sorted(found)}")
    for name, s in found.items():
        if s["reasons"] != [f"no {CONFIG} at {commit}"]:
            problems.append(f"{name}: {s['reasons']}")
    T.check(
        "no config: exit 1, every config signal names the absence, the file signals pass",
        not problems,
        "; ".join(problems),
    )
    return commit


# --- unusable input -----------------------------------------------------------


def unusable(sb: Sandbox, name: str, args: list, fragment: str, env: dict = None, script: Path = CHECK) -> None:
    proc, data = sb.run(args, env=env, script=script, verdict="--json" not in args)
    lines = text(proc.stderr).split("\n")
    problems = []
    if proc.returncode != 2:
        problems.append(f"exit {proc.returncode}")
    if len(lines) != 2 or lines[1] != "" or not lines[0].startswith("readiness: ") or fragment not in lines[0]:
        problems.append(f"stderr {text(proc.stderr)!r} lacks {fragment!r}")
    if proc.stdout:
        problems.append(f"stdout {text(proc.stdout)!r}")
    if data is not None:
        problems.append("a verdict file is left")
    T.check(f"unusable: {name}: exit 2 naming the cause, no verdict", not problems, "; ".join(problems))


def check_unusable(sb: Sandbox, ready: str) -> None:
    stale = str(sb.out / "stale.json")
    unusable(sb, "no --ref", ["--json", stale], "missing --ref")
    unusable(sb, "--ref twice", ["--ref", ready, "--ref", ready, "--json", stale], "--ref given twice")
    unusable(sb, "an unknown argument", ["--ref", ready, "--bogus", "--json", stale], "unknown argument '--bogus'")
    unusable(sb, "--ref -x", ["--ref", "-x", "--json", stale], "--ref must not start with '-'")
    unusable(sb, "--json without a value", ["--ref", ready, "--json", "--run-gates"], "--json needs a value")
    unusable(sb, "a missing ref", ["--ref", "no-such-ref"], "ref 'no-such-ref' does not resolve to a commit")
    unusable(sb, "an unborn HEAD", ["--ref", "HEAD"], "ref 'HEAD' does not resolve to a commit")
    rejected = sb.fixture(m_file(CONFIG, b"tracker: [\n"))
    unusable(sb, "YAML yq rejects", ["--ref", rejected], "yq")
    listed = sb.fixture(m_file(CONFIG, b"- tracker\n- gates\n"))
    unusable(sb, "YAML that is a list", ["--ref", listed], "is not a mapping")
    linked = sb.fixture(m_file(CONFIG, (SYMLINK, b"other.yaml")))
    unusable(sb, "a symlinked config", ["--ref", linked], "is not a regular file")
    bin_dir = sb.tmp / "bin-git-only"
    bin_dir.mkdir()
    os.symlink(shutil.which("git"), bin_dir / "git")
    unusable(sb, "yq missing from PATH", ["--ref", ready], "cannot run yq", env=dict(sb.env, PATH=str(bin_dir)))
    lonely = sb.tmp / "lonely" / "readiness"
    lonely.mkdir(parents=True)
    shutil.copyfile(CHECK, lonely / "check.py")
    unusable(
        sb,
        "no ../plan-approval/floor.py",
        ["--ref", ready],
        "plan floor ../plan-approval/floor.py",
        script=lonely / "check.py",
    )


# --- worktree and determinism ---------------------------------------------------


def check_worktree(sb: Sandbox, ready: str, first) -> None:
    written = {CONFIG: b"gates: {tests: none}\n", "AGENTS.md": b"changed\n", ".claude/agents/msw-probe.md": b"x\n"}
    for path, data in written.items():
        (sb.repo / path).parent.mkdir(parents=True, exist_ok=True)
        (sb.repo / path).write_bytes(data)
    proc, data = sb.run(["--ref", "ready"])
    T.check(
        "worktree: files that differ from the ref leave the output unchanged",
        proc.returncode == 0 and proc.stdout == first[0].stdout and data == first[1],
        text(proc.stdout),
    )
    for path in written:
        (sb.repo / path).unlink()
    shutil.rmtree(sb.repo / ".mintswarm")
    shutil.rmtree(sb.repo / ".claude")


def check_determinism(sb: Sandbox, refs: list) -> None:
    for name, ref in refs:
        one = sb.run(["--ref", ref])
        two = sb.run(["--ref", ref])
        T.check(
            f"determinism: two runs on {name} give the same bytes",
            one[0].stdout == two[0].stdout and one[1] == two[1] and one[1] is not None,
            text(one[0].stdout),
        )


# --- the gate-running option ------------------------------------------------------


def gate_ids(names: list) -> list:
    return [("gates", f"gate:{name}") for name in names]


def clean_state(sb: Sandbox, before: str) -> list:
    problems = []
    if sb.worktrees() != before:
        problems.append(f"worktrees {sb.worktrees()!r}")
    if os.listdir(sb.checktmp):
        problems.append(f"TMPDIR holds {os.listdir(sb.checktmp)}")
    return problems


def group_gone(pid: int) -> bool:
    for _ in range(50):
        try:
            os.killpg(pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            return False
        time.sleep(0.1)
    return False


def process_gone(pid: int) -> bool:
    for _ in range(50):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            return False
        time.sleep(0.1)
    return False


def read_pid(sb: Sandbox, wait: float = 0.0, variable: str = PIDFILE):
    path = Path(sb.env[variable])
    deadline = time.monotonic() + wait
    while True:
        content = path.read_text(encoding="utf-8") if path.exists() else ""
        if content.endswith("\n") and content.strip().isdigit():
            return int(content)
        if time.monotonic() >= deadline:
            return None
        time.sleep(0.05)


def check_run_gates(sb: Sandbox, ready: str) -> None:
    before = sb.worktrees()
    one = sb.run(["--ref", ready, "--run-gates"])
    problems = shape_problems(sb, *one, ready, ready, gate_ids(["unit", "drift", "shell"]))
    problems += clean_state(sb, before)
    T.check(
        "run-gates: the ready fixture's gates pass as gate:<name> signals after the static ones",
        one[0].returncode == 0 and not problems,
        "; ".join(problems) or text(one[0].stdout),
    )
    two = sb.run(["--ref", ready, "--run-gates"])
    T.check("run-gates: two runs give the same bytes", one[0].stdout == two[0].stdout and one[1] == two[1])

    gates = [
        {"name": "ok", "command": "python3 tests/test_ok.py"},
        {"name": "fails", "command": "exit 3"},
        {"name": "slow", "command": "python3 tests/slow.py", "timeout": 1},
        {"name": "writes", "command": "touch marker"},
        {"name": "fresh", "command": "if test -e marker; then exit 1; fi"},
        {"name": "environment", "command": "python3 tests/environment.py"},
        {"name": "background", "command": "python3 tests/background.py", "timeout": 20},
        {"name": "readonly", "command": "chmod -R a-w . && chmod a-w .."},
    ]
    commit = sb.fixture(with_gates(gates))
    for path in (sb.env[PIDFILE], sb.env[GATE_OUT]):
        if os.path.exists(path):
            os.unlink(path)
    proc, data = sb.run(["--ref", commit])
    T.check(
        "run-gates: without the option no gate runs",
        proc.returncode == 0 and not os.path.exists(sb.env[GATE_OUT]) and not os.path.exists(sb.env[PIDFILE]),
        text(proc.stdout),
    )
    started = time.monotonic()
    proc, data = sb.run(["--ref", commit, "--run-gates"])
    elapsed = time.monotonic() - started
    problems = shape_problems(sb, proc, data, commit, commit, gate_ids([g["name"] for g in gates]))
    problems += clean_state(sb, before)
    found = {name: s["reasons"] for name, s in failing(data).items()}
    if found != {"gate:fails": ["exit code 3"], "gate:slow": ["timed out after 1 s"]}:
        problems.append(f"failing {found}")
    if elapsed > 20:
        problems.append(f"took {elapsed:.0f} s")
    T.check("run-gates: a failing gate, a timeout and a fresh worktree per gate", not problems, "; ".join(problems))
    pid = read_pid(sb)
    T.check("run-gates: the timed-out gate's process group is gone", pid is not None and group_gone(pid), f"{pid}")
    T.check("run-gates: gates get the environment the check was started with", os.path.exists(sb.env[GATE_OUT]))
    background = read_pid(sb, variable=BACKGROUND)
    T.check(
        "run-gates: what a gate leaves running in the background is killed when it exits",
        background is not None and process_gone(background),
        f"{background}",
    )
    check_git_environment(sb, before)

    commit = sb.fixture(m_set("gates", "run", []))
    proc, data = sb.run(["--ref", commit, "--run-gates"])
    problems = shape_problems(sb, proc, data, commit, commit, [("gates", "gate-run")])
    found = failing(data)
    if set(found) != {"gates-run", "tests-gated", "gate-run"}:
        problems.append(f"failing {sorted(found)}")
    T.check("run-gates: with no gate to run a single gate-run signal fails", not problems, "; ".join(problems))

    check_sigterm(sb, before)


def check_git_environment(sb: Sandbox, before: str) -> None:
    where = "python3 tests/test_ok.py && python3 tests/where.py"
    commit = sb.fixture(with_gates([{"name": "where", "command": where}]))
    if os.path.exists(sb.env[GATE_OUT]):
        os.unlink(sb.env[GATE_OUT])
    index = sb.repo / ".git" / "index"
    existed = index.exists()
    env = dict(sb.env, GIT_DIR=str(sb.repo / ".git"), GIT_WORK_TREE=str(sb.repo), GIT_INDEX_FILE=str(index))
    proc, data = sb.run(["--ref", commit, "--run-gates"], env=env)
    problems = shape_problems(sb, proc, data, commit, commit, gate_ids(["where"]))
    problems += clean_state(sb, before)
    out = Path(sb.env[GATE_OUT]).read_text(encoding="utf-8") if os.path.exists(sb.env[GATE_OUT]) else None
    if out != commit + "\n":
        problems.append(f"the gate saw {out!r}, not a clean worktree at {commit}")
    if index.exists() and not existed:
        problems.append("the checked repository's index was written")
    T.check(
        "run-gates: with GIT_DIR, GIT_WORK_TREE and GIT_INDEX_FILE set, a gate's git works on its own worktree",
        proc.returncode == 0 and not problems,
        "; ".join(problems),
    )


def check_sigterm(sb: Sandbox, before: str) -> None:
    commit = sb.fixture(with_gates([{"name": "hang", "command": "python3 tests/slow.py", "timeout": 60}]))
    if os.path.exists(sb.env[PIDFILE]):
        os.unlink(sb.env[PIDFILE])
    path = sb.out / "sigterm.json"
    path.write_text('{"stale": true}\n', encoding="utf-8")
    proc = subprocess.Popen(
        [sys.executable, "-I", str(CHECK), "--ref", commit, "--run-gates", "--json", str(path)],
        cwd=sb.repo,
        env=sb.env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    pid = read_pid(sb, wait=20)
    if pid is not None:
        proc.send_signal(signal.SIGTERM)
    try:
        out, err = proc.communicate(timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        out, err = proc.communicate()
    problems = clean_state(sb, before)
    if pid is None:
        problems.append("the gate never started")
    if proc.returncode != 2 or "readiness: terminated by SIGTERM" not in text(err) or out:
        problems.append(f"exit {proc.returncode}, stderr {text(err)!r}, stdout {text(out)!r}")
    if path.exists():
        problems.append("a verdict file is left")
    if pid is not None and not group_gone(pid):
        problems.append("the gate's process group is still running")
    T.check(
        "run-gates: SIGTERM during a gate exits 2 and leaves no process, worktree or directory",
        not problems,
        "; ".join(problems),
    )


# --- standard library -------------------------------------------------------------


def check_sources() -> None:
    for path in (CHECK, Path(__file__).resolve()):
        source = path.read_text(encoding="utf-8")
        tree = ast.parse(source)
        names = set()
        unions = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                names.add("." if node.level else node.module.split(".")[0])
            annotations = []
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                arguments = node.args.posonlyargs + node.args.args + node.args.kwonlyargs
                annotations = [a.annotation for a in arguments] + [node.returns]
            elif isinstance(node, ast.AnnAssign):
                annotations = [node.annotation]
            for annotation in annotations:
                if annotation is not None and any(
                    isinstance(n, ast.BinOp) and isinstance(n.op, ast.BitOr) for n in ast.walk(annotation)
                ):
                    unions.append(annotation.lineno)
        outside = sorted(n for n in names if n not in STDLIB)
        T.check(
            f"standard library: {path.name} imports only {', '.join(sorted(names))}",
            not outside,
            f"not in the allowlist: {outside}",
        )
        try:
            ast.parse(source, feature_version=(3, 9))
            parsed = True
        except SyntaxError:
            parsed = False
        T.check(
            f"python 3.9: {path.name} parses as 3.9 with no X | Y annotations",
            parsed and not unions,
            f"union annotations on lines {unions}",
        )


def main() -> int:
    for tool in ("git", "yq", "bash"):
        if shutil.which(tool) is None:
            print(f"selftest.py: {tool} is not on PATH", file=sys.stderr)
            return 2
    tmp = tempfile.mkdtemp(prefix="msw-readiness-selftest-")
    try:
        sb = Sandbox(Path(tmp))
        ready, *first = check_ready(sb)
        check_gaps(sb)
        check_ready_variants(sb)
        check_script_tools(sb)
        no_config = check_no_config(sb)
        check_unusable(sb, ready)
        check_worktree(sb, ready, first)
        gaps = sb.fixture(m_all(m_file("stray.txt", b"x\n"), m_set("gates", "tests", "none"), m_rh_landing))
        check_determinism(
            sb, [("the ready fixture", ready), ("a gaps fixture", gaps), ("a ref without a config", no_config)]
        )
        check_run_gates(sb, ready)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    check_sources()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
