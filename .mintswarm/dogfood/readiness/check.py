#!/usr/bin/env python3
"""Report what each mintswarm pipeline feature is missing in a repository, at one ref.

    check.py --ref REF [--json PATH] [--run-gates]

Dogfooding stand-in for the readiness check of onboarding: a deterministic
list of binary signals, grouped by the pipeline feature each one blocks, so
that onboarding plans only the gaps. It reads the repository at the working
directory, at the commit REF resolves to, through git alone: the file list
with `git ls-tree -r -z --full-tree`, blobs with `git cat-file blob`, and
.mintswarm/config.yaml with `git show` converted by `yq -p=yaml -o=json`, as
the plan floor reads it. The worktree is never read. Every git command runs
with the landing floor's neutral configuration (no system, global or
environment config, replace objects and grafts off).

--ref is required, --json and --run-gates optional; each is given at most
once. Anything else, an empty value, a value that is an argument name or a
REF starting with `-` is unusable input. The first action is to delete any
stale file at each PATH that follows a `--json` in the arguments.

The plan floor (../plan-approval/floor.py), the landing floor
(../landing/floor.py), ../report-gates.py, ../plan-approval/decision.py and
../render-agents.py are loaded by file path from beside this script, and
with --run-gates ../run-gates.py too, so a readiness pass means the same
validation as theirs. A missing or broken one is unusable input.

Signals, evaluated in this order. A signal marked * reads the config, and
fails with `no .mintswarm/config.yaml at <commit>` when the ref has none.
Globs match as in the plan floor; tracked paths are every ls-tree entry.

  feature             signal                when it fails
  tracker             tracker-kinds *       tracker.kind is not a non-empty
                                            string; tracker.kinds.source is
                                            not issue_types or labels; design,
                                            plan or implementation lacks a
                                            non-empty issue_type (or label,
                                            under labels); tracker.components
                                            .source is not labels or field, or
                                            under labels its label lacks
                                            {name}
  ingest              trusted-ingest *      tracker.ingest.trusted.labels or
                                            .authors is not a list of strings,
                                            tracker.ingest.match is not any or
                                            all, both lists are empty, or one
                                            is empty under all
  components          components-cover *    a tracked path matches no
                                            component's paths and no shared
                                            glob, or two components, or a
                                            component and a shared glob
                                            (case-sensitive); each is named in
                                            tree order
  review              codeowners            the first of .github/CODEOWNERS,
                                            CODEOWNERS and docs/CODEOWNERS
                                            that exists is not a regular file
                                            or has no rule naming an owner, or
                                            none exists
  review              reviewers *           a reviewers list is not a list of
                                            logins ([A-Za-z0-9-]+, a leading @
                                            dropped), or a tracked path gets
                                            none: the union of its components'
                                            reviewers, else
                                            pipeline.review.reviewers, as
                                            github/request-review.py resolves
                                            them
  agent-instructions  agents-md             AGENTS.md is not a regular file
  agent-instructions  claude-md             CLAUDE.md is neither a symbolic
                                            link to AGENTS.md nor a regular
                                            file with a line `@AGENTS.md`
  agent-instructions  contributing          CONTRIBUTING.md is not a regular
                                            file
  agent-instructions  architecture-docs     no tracked path under
                                            docs/architecture/
  agent-instructions  agents-drift          the checker's own renderer, run
                                            over a temporary copy of the ref's
                                            .mintswarm/stages/*.md and
                                            .claude/agents/msw-*.md, reports
                                            a source error or drift (its
                                            missing:, differs: and orphan:
                                            lines, sorted), or one of those
                                            paths is not a regular file;
                                            passes as not applicable when the
                                            ref has no
                                            .mintswarm/dogfood/render-agents.py,
                                            and notes when the ref's renderer
                                            differs from the checker's
  gates               gates-run *           gates.run is not a non-empty list
                                            of mappings with non-empty `name`
                                            and `command` strings, the names
                                            unique
  gates               gate-scripts-named *  a gate command, read as below,
                                            cannot be read, or a simple
                                            command of it runs code that no
                                            tracked file named in the command
                                            holds
  gates               gate-tools *          a gate command cannot be read,
                                            or bash, or the command word of a
                                            gate's simple command that is not
                                            a path or a shell builtin, is not
                                            found on PATH (only the name is
                                            printed)
  gates               tests-gated *         a test file is not a word of any
                                            gate command (shlex words, a
                                            leading ./ dropped, each also
                                            joined to the cd directories
                                            before it, as the plan floor's
                                            gate words)
  gate-reports        report-formats *      report-gates.py's resolve rejects
                                            gates.report
  plan-approval       plan-approval-config * the plan floor's validate_config
                                            rejects the config, given its
                                            first component by name
  plan-approval       docs-component *      components.docs has no paths
  plan-approval       tests-exist           no tracked test file
  plan-approval       tests-declared *      gates.tests is `none` while test
                                            files exist
  plan-approval       require-human-plan *  a protected path is not matched,
                                            case-insensitively, by
                                            pipeline.plan_approval
                                            .require_human paths or by the
                                            paths of a component it lists
  plan-approval       assess-model *        the assess model (as decision.py's
                                            expected_judge reads it) or the
                                            plan model (agents.stages.plan
                                            .model over agents.default.model)
                                            is not a string, or they are equal
  landing             landing-config *      the landing floor's
                                            validate_landing rejects
                                            pipeline.landing
  landing             require-human-landing * an agent instruction is not
                                            matched by pipeline.landing
                                            .require_human in the same way

Test files are tracked blobs whose base name matches `*selftest*`,
`test_*.py`, `*_test.*`, `*.test.*` or `*.spec.*`, or that are under a
directory named `tests`. The agent instructions are AGENTS.md, CLAUDE.md and
the tracked paths that are .mintswarm/stages.md or under .claude/agents/ or
.mintswarm/stages/. The protected paths of require-human-plan are
.mintswarm/config.yaml, each tracked file a gate command names and the agent
instructions; landing's fixed `protected` rule already covers the config and
the gate words, so require-human-landing checks the agent instructions only.

Gate commands are read by a small allow-list grammar over their raw text,
not by modelling bash's lexer, and no shlex is used, so what is read is
what bash reads. A command outside the grammar is unreadable, and an
unreadable command fails both gate-scripts-named and gate-tools. The
grammar:

- Characters: the raw text holds only ASCII letters and digits, space,
  newline and `._/:=+,@%-`, which make words, and `;`, `|`, `&` and `>`,
  which make operators and redirections. Any other character is
  unreadable: quotes of any kind, backslash, tab, CR, `{}[]()*?~!$#<`,
  backtick and the rest. So nothing in a command is quoted, escaped,
  expanded, substituted, grouped or commented out.
- Splitting: the text is split on spaces and newlines, then each piece on
  the operators `&&`, `||`, `;` and `|`. Newline, `;`, `&&`, `||` and `|`
  end a simple command; `;`, `&&`, `||` and `|` must follow a word and
  must not end the command. A piece that is exactly `>/dev/null`,
  `2>/dev/null`, `2>&1` or `>&2` is a redirection and is dropped; any other
  piece holding `>` or `&` is unreadable. Every other piece is a word as
  written, digits included.
- A simple command is any of the reserved words `if then else elif fi
  while until do done`, then the command word and its arguments.
- The reader models no options of any program: it never skips an option or
  an option's argument to find a command word or a script. So the prefixes
  env, exec, time and command, which take options and then run another
  word, are unreadable, and so is a command word that holds `=` (an
  assignment, `NAME+=` and env's rule alike) or starts with `+`.
- A command word that is a bash builtin or keyword must be one of `:`,
  `true`, `false`, `echo`, `test`, `pwd`, `exit`, `shift` and `wait`, which
  evaluate nothing whatever their arguments; `source` and `.` are read as
  interpreters, below. `[` is outside the characters, so a test is written
  with `test`. cd is read as below. Every other builtin or keyword (set,
  printf, export, declare, typeset, local, readonly, read, unset, let,
  eval, trap, builtin, compgen, complete, bind, for, case, function, coproc
  and the rest) is unreadable.
- cd is readable only as `cd DIR` in a command whose simple commands are
  joined by `&&` alone, each `&&` a word of its own, with no newline, no
  reserved word and no redirection in the cd itself: a failed cd then
  stops the command, and shlex (the floors' gate words) splits the text
  into the same words as the reader. DIR is `.` or starts with `./`, so
  bash never looks it up in CDPATH, and has no empty or `..` segment.
  Taken from the directory the cds before it lead to, with `.` segments
  dropped, DIR must be the root or a directory that holds a tracked blob,
  so it is tracked and no symbolic link is on its path. Every later
  interpreter script and command word holding `/` (unless it starts with
  `/`) is resolved against that directory, and the reasons name the
  resolved path. The floors' gate words join each word to the cd
  directories before it, so they protect exactly the path read here.
- An interpreter the reader knows (bash, sh, zsh, dash, python, python3,
  node, perl, ruby, source, `.`) is readable only as `INTERPRETER SCRIPT
  [ARGUMENTS...]`: the word right after it is its script. An interpreter
  given no word, or a word starting with `-` or `+` (an option of any
  kind, `-` itself included), is unreadable.

A readable simple command fails gate-scripts-named when:

- it runs a wrapper that runs another command (timeout, nice, nohup,
  stdbuf, xargs, sudo, doas, su, setsid, flock, watch, parallel, chroot,
  ionice, taskset, caffeinate, unbuffer, script, nsenter, unshare, npx,
  bunx), find with -exec, -execdir, -ok or -okdir, a task runner (make,
  gmake, just, yarn, pnpm, rake, tox, nox, invoke), or npm run, run-script,
  exec, x, test, t, start, restart or stop, bun run or x, uv, poetry, pdm,
  hatch or pipenv run, or bundle exec;
- it runs an interpreter the reader does not model: a versioned name such
  as python3.12, or awk, gawk, mawk, nawk, ksh, mksh, ash, fish, csh, tcsh,
  busybox, pypy, pypy3, python2, nodejs, deno, bun, php, lua, luajit,
  tclsh, wish, Rscript, osascript, expect, pwsh, ts-node or tsx;
- it runs an interpreter the reader knows whose script is not a tracked
  regular file;
- its command word holds `/` and is not a tracked regular file, unless it
  is not a tracked blob either and, resolved, is `[DIR/]node_modules/.bin/
  NAME`, where no segment of DIR is empty, `.`, `..` or `node_modules`,
  NAME is one segment other than `.` and `..`, and a tracked regular
  `pnpm-lock.yaml` or `package-lock.json` sits in DIR or a directory
  above it. Such a binary is dependency code pinned by integrity in a
  tracked lockfile, which the dependency review judges, as cargo's
  dependencies are; a workspace package's own bin linked there is tracked
  code the floors do not protect, a known limit.

`pnpm install` stays unreadable as a task runner: it runs a .pnpmfile.cjs,
lifecycle scripts of package.json files the floors do not protect and
dependency build scripts, and reading it would mean modelling pnpm's
options. A gate installs through a tracked script instead, which the floors
then protect.

A tracked symbolic link is never a tracked regular file, since it can point
outside the repository. Any other command word is taken as a tool that runs
its own code, so a tool that itself runs an inline program (sed's `e`
command, a git alias, a compiler plugin) is not followed. The tests-gated
signal reads gate words as the plan floor does, with shlex.split.

With --run-gates, after the static signals, each gate of gates.run that
gates-run accepts (the first of each name), in order, runs as
`bash -c <command>` in its own session: in a fresh detached worktree of
the commit, added under a new temporary directory and removed right after
the gate (owner write permission restored on the tree, then
`git worktree remove --force --force`, then the directory). Its stdin,
stdout and stderr are the null device. Its environment is the one the check
was started with, minus git's repository-local variables (GIT_DIR,
GIT_WORK_TREE, GIT_IMPLICIT_WORK_TREE, GIT_INDEX_FILE, GIT_OBJECT_DIRECTORY,
GIT_ALTERNATE_OBJECT_DIRECTORIES, GIT_COMMON_DIR, GIT_PREFIX, GIT_NAMESPACE,
GIT_CONFIG, GIT_CONFIG_PARAMETERS, GIT_CONFIG_COUNT, GIT_CONFIG_KEY_n,
GIT_CONFIG_VALUE_n, GIT_GRAFT_FILE, GIT_NO_REPLACE_OBJECTS,
GIT_REPLACE_REF_BASE, GIT_SHALLOW_FILE), so its git commands work on its
own worktree. The worktree is added with GIT_INDEX_FILE, GIT_WORK_TREE and
GIT_PREFIX dropped and hooks disabled; the repository's own configured
filters (smudge, process) still run on its checkout, chosen by the ref's
.gitattributes, which the static signals never do. The timeout is the
gate's `timeout` when it is a positive integer, otherwise run-gates.py's
DEFAULT_TIMEOUT_S. Whether the gate exits, times out or the check is
stopped, its whole process group is then killed with run-gates.py's
kill_group, so nothing it started in the background outlives it (a process
that starts its own session escapes this). Signal `gates gate:<name>`
passes on exit 0 and otherwise gives `exit code N` or `timed out after N s`.
With no such gate, one signal `gates gate-run` fails. SIGTERM while the
gates run ends the gate's process group, removes its worktree and exits 2.
No `git worktree prune` runs.

The verdict, written to PATH when --json is given (to a temporary file next
to it, then renamed), has these keys in this order: `tool`
(`mintswarm-readiness`), `version` (1), `ref` ({`ref` as given, `commit`, the
full hash}), `config` (`.mintswarm/config.yaml`, or null when the ref has
none), `verdict` (`ready` or `gaps`) and `signals` (one {`feature`,
`signal`, `result`, `reasons`} per signal, in the order above, then the gate
signals; `result` is `pass` or `fail`, and `reasons` may be non-empty on a
pass). One line per signal is printed, `<result> <feature> <signal>`,
the result padded to 7 characters, with its first reason, if any, after a
colon and control characters escaped as the landing floor does; then
`readiness: ready` or `readiness: gaps`. Nothing printed or written holds a
time, a duration, a temporary path or an environment value, so two runs on
the same ref in the same environment give the same bytes.

Exits 0 when every signal passes, 1 when any fails, 2 on unusable input,
naming the cause on stderr as `readiness: <cause>`: bad arguments, a stale
verdict that cannot be deleted, a missing or broken helper script, a ref
that does not resolve to a commit, a tracked path that is not UTF-8, a
config that is not a regular file, that yq rejects or that is not a strict
JSON mapping, git or yq missing or failing, a gate worktree that cannot be
added or removed, SIGTERM during the gates, or any unexpected error. Exit 2
leaves no verdict. Standard library only, apart from running git, yq and,
with --run-gates, bash.
"""

import contextlib
import importlib.util
import io
import json
import os
import re
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
from fnmatch import fnmatchcase
from pathlib import Path

HERE = Path(__file__).resolve().parent
DOGFOOD = HERE.parent
HELPERS = (
    ("plan floor", "plan-approval/floor.py"),
    ("landing floor", "landing/floor.py"),
    ("report-gates.py", "report-gates.py"),
    ("decision.py", "plan-approval/decision.py"),
    ("render-agents.py", "render-agents.py"),
)
RUN_GATES_HELPER = ("run-gates.py", "run-gates.py")
TOOL = "mintswarm-readiness"
VERSION = 1
CONFIG = ".mintswarm/config.yaml"
RENDERER = ".mintswarm/dogfood/render-agents.py"
STAGE_SOURCES = ".mintswarm/stages/"
RENDERED_AGENTS = ".claude/agents/"
STAGES_FILE = ".mintswarm/stages.md"
ARCHITECTURE = "docs/architecture/"
REGULAR = ("100644", "100755")
SYMLINK = "120000"
CODEOWNERS = (".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS")
LOGIN = re.compile(r"[A-Za-z0-9-]+")
OWNER = re.compile(r"@\S+|[^@\s]+@[^@\s]+")
TEST_NAMES = ("*selftest*", "test_*.py", "*_test.*", "*.test.*", "*.spec.*")
KIND_KEYS = {"issue_types": "issue_type", "labels": "label"}
DOCS = "docs"
ARGS = ("--ref", "--json", "--run-gates")
VALUED = ("--ref", "--json")
USAGE = "usage: check.py --ref REF [--json PATH] [--run-gates]"

INTERPRETERS = frozenset("bash sh zsh dash python python3 node perl ruby source .".split())
PREFIXES = frozenset("env exec time command".split())
UNMODELLED = frozenset(
    """
    awk gawk mawk nawk ksh mksh ash fish csh tcsh busybox pypy pypy3 python2 nodejs deno bun php lua luajit tclsh
    wish Rscript osascript expect pwsh ts-node tsx
    """.split()
)
VERSIONED = re.compile(r"(?:python|pypy|perl|ruby|node|php|lua|bash|zsh|ksh|tclsh)[0-9][0-9.]*")
WRAPPERS = frozenset(
    """
    timeout nice nohup stdbuf xargs sudo doas su setsid flock watch parallel chroot ionice taskset caffeinate
    unbuffer script nsenter unshare npx bunx
    """.split()
)
FIND_EXEC = ("-exec", "-execdir", "-ok", "-okdir")
TASK_RUNNERS = frozenset("make gmake just yarn pnpm rake tox nox invoke".split())
SUBCOMMAND_RUNNERS = {
    "npm": ("run", "run-script", "exec", "x", "test", "t", "start", "restart", "stop"),
    "bun": ("run", "x"),
    "uv": ("run",),
    "poetry": ("run",),
    "pdm": ("run",),
    "hatch": ("run",),
    "pipenv": ("run",),
    "bundle": ("exec",),
}
WORD = re.compile(r"[A-Za-z0-9._/:=+,@%-]+")
COMMAND_CHARACTERS = frozenset("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/:=+,@%- \n;|&>")
OPERATORS = re.compile(r"(&&|\|\||;|\|)")
REDIRECTIONS = (">/dev/null", "2>/dev/null", "2>&1", ">&2")
ALLOWED_BUILTINS = frozenset(": true false echo test pwd exit shift wait".split())
RESERVED = frozenset("if then else elif fi do done while until".split())
CD = "cd"
LOCKFILES = ("pnpm-lock.yaml", "package-lock.json")
# Every bash builtin and keyword; outside ALLOWED_BUILTINS, and apart from the prefixes, source and `.`, unreadable.
BUILTINS = frozenset(
    """
    : . [ [[ ]] (( )) alias bg bind break builtin caller case cd command compgen complete compopt continue coproc
    declare dirs disown do done echo elif else enable esac eval exec exit export false fc fg fi for function getopts
    hash help history if in jobs kill let local logout mapfile popd printf pushd pwd read readarray readonly return
    select set shift shopt source suspend test then time times trap true type typeset ulimit umask unalias unset
    until wait while { } !
    """.split()
)
# git's repository-local variables (git rev-parse --local-env-vars, and GIT_NAMESPACE), kept from the gates.
GIT_LOCAL_ENV = (
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
)
GIT_LOCAL_PREFIXES = ("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")
WORKTREE_DROPPED_ENV = ("GIT_INDEX_FILE", "GIT_WORK_TREE", "GIT_PREFIX")

SIGNALS = (
    ("tracker", "tracker-kinds", True),
    ("ingest", "trusted-ingest", True),
    ("components", "components-cover", True),
    ("review", "codeowners", False),
    ("review", "reviewers", True),
    ("agent-instructions", "agents-md", False),
    ("agent-instructions", "claude-md", False),
    ("agent-instructions", "contributing", False),
    ("agent-instructions", "architecture-docs", False),
    ("agent-instructions", "agents-drift", False),
    ("gates", "gates-run", True),
    ("gates", "gate-scripts-named", True),
    ("gates", "gate-tools", True),
    ("gates", "tests-gated", True),
    ("gate-reports", "report-formats", True),
    ("plan-approval", "plan-approval-config", True),
    ("plan-approval", "docs-component", True),
    ("plan-approval", "tests-exist", False),
    ("plan-approval", "tests-declared", True),
    ("plan-approval", "require-human-plan", True),
    ("plan-approval", "assess-model", True),
    ("landing", "landing-config", True),
    ("landing", "require-human-landing", True),
)

P = None  # the plan floor's module, once loaded
L = None  # the landing floor's module
RG = None  # report-gates.py
D = None  # plan-approval/decision.py
R = None  # render-agents.py
G = None  # run-gates.py, with --run-gates only


class InputError(Exception):
    """Unusable input: exit 2, no verdict."""


class CommandError(Exception):
    """A gate command that the check cannot read."""


# --- arguments and helpers -------------------------------------------------


def stale_targets(argv: list) -> list:
    """Every PATH that follows a `--json`, even in arguments that do not parse."""
    return [argv[i + 1] for i, name in enumerate(argv[:-1]) if name == "--json" and argv[i + 1] not in ("", *ARGS)]


def remove(path) -> None:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def remove_stale(argv: list) -> None:
    for path in stale_targets(argv):
        try:
            remove(path)
        except OSError as exc:
            raise InputError(f"cannot delete the stale verdict {path}: {exc}") from exc


def parse_args(argv: list) -> dict:
    args = {}
    i = 0
    while i < len(argv):
        name = argv[i]
        if name not in ARGS:
            raise InputError(f"unknown argument {name!r}; {USAGE}")
        if name in args:
            raise InputError(f"{name} given twice; {USAGE}")
        if name not in VALUED:
            args[name] = True
            i += 1
            continue
        if i + 1 >= len(argv) or argv[i + 1] == "" or argv[i + 1] in ARGS:
            raise InputError(f"{name} needs a value; {USAGE}")
        args[name] = argv[i + 1]
        i += 2
    if "--ref" not in args:
        raise InputError(f"missing --ref; {USAGE}")
    if args["--ref"].startswith("-"):
        raise InputError(f"--ref must not start with '-': {args['--ref']!r}")
    return {"ref": args["--ref"], "json": args.get("--json"), "run_gates": "--run-gates" in args}


def load(what: str, relative: str, name: str):
    path = DOGFOOD / relative
    if not path.is_file():
        raise InputError(f"the {what} ../{relative} next to this script's directory is missing")
    sys.dont_write_bytecode = True  # leave no __pycache__ next to the helpers
    try:
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception as exc:
        message = " ".join(str(exc).split())
        raise InputError(f"cannot load the {what} ../{relative}: {type(exc).__name__}: {message}") from exc
    return module


def load_helpers(run_gates: bool) -> None:
    global P, L, RG, D, R, G
    P = load(*HELPERS[0], "msw_readiness_plan_floor")
    L = load(*HELPERS[1], "msw_readiness_landing_floor")
    L.P = P  # validate_landing reads the plan floor through this global
    RG = load(*HELPERS[2], "msw_readiness_report_gates")
    D = load(*HELPERS[3], "msw_readiness_decision")
    R = load(*HELPERS[4], "msw_readiness_render_agents")
    if run_gates:
        G = load(*RUN_GATES_HELPER, "msw_readiness_run_gates")


def brief(value) -> str:
    return L.brief(value)


def outcome(reasons: list) -> tuple:
    return ("fail" if reasons else "pass"), reasons


def dig(value, *keys):
    """The value at keys in nested mappings, or None when a level is missing or not a mapping."""
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def is_text(value) -> bool:
    return isinstance(value, str) and value != ""


def is_string_list(value) -> bool:
    return isinstance(value, list) and all(isinstance(v, str) for v in value)


def strip_dot(word: str) -> str:
    while word.startswith("./"):
        word = word[2:]
    return word


# --- the ref ---------------------------------------------------------------


def resolve(ref: str) -> str:
    try:
        return P.resolve_base(ref)
    except P.InputError as exc:
        if "does not resolve" in str(exc):
            raise InputError(f"ref {ref!r} does not resolve to a commit") from exc
        raise


def ls_tree(commit: str) -> list:
    """[(path, mode, type, object)] for every entry at the commit, in tree order."""
    data = P.run(["git", "ls-tree", "-r", "-z", "--full-tree", commit])
    if data and not data.endswith(b"\0"):
        raise InputError("git ls-tree: output does not end in NUL")
    entries = []
    for raw in data[:-1].split(b"\0") if data else []:
        header, tab, path = raw.partition(b"\t")
        parts = header.decode("ascii", errors="replace").split(" ")
        if not tab or not path or len(parts) != 3:
            raise InputError(f"git ls-tree: unexpected record {raw!r}")
        try:
            name = path.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise InputError(f"a tracked path at {commit} is not UTF-8: {path!r}") from exc
        entries.append((name, parts[0], parts[1], parts[2]))
    return entries


class Ref:
    def __init__(self, commit: str, entries: list, config):
        self.commit = commit
        self.entries = entries
        self.by_path = {path: (mode, kind, oid) for path, mode, kind, oid in entries}
        self.paths = [path for path, _, _, _ in entries]
        self.blobs = {path for path, _, kind, _ in entries if kind == "blob"}
        self.regular_files = {path for path, mode, kind, _ in entries if kind == "blob" and mode in REGULAR}
        self.directories = {""} | {path[:i] for path in self.blobs for i, c in enumerate(path) if c == "/"}
        self.config = config
        self.tests = [path for path in self.paths if path in self.blobs and is_test(path)]
        self._cache = {}

    def blob(self, path: str) -> bytes:
        oid = self.by_path[path][2]
        if oid not in self._cache:
            self._cache[oid] = P.run(["git", "cat-file", "blob", oid])
        return self._cache[oid]

    def regular(self, path: str) -> bool:
        entry = self.by_path.get(path)
        return entry is not None and entry[0] in REGULAR

    def instructions(self) -> list:
        """The agent instructions: AGENTS.md, CLAUDE.md and the tracked stage files and rendered agents."""
        found = ["AGENTS.md", "CLAUDE.md"]
        for path in self.paths:
            if path == STAGES_FILE or path.startswith((RENDERED_AGENTS, STAGE_SOURCES)):
                found.append(path)
        return found


def is_test(path: str) -> bool:
    segments = path.split("/")
    return "tests" in segments[:-1] or any(fnmatchcase(segments[-1], name) for name in TEST_NAMES)


def read_config(commit: str, by_path: dict):
    entry = by_path.get(CONFIG)
    if entry is None:
        return None
    if entry[0] not in REGULAR:
        raise InputError(f"{CONFIG} at {commit} is not a regular file (mode {entry[0]})")
    return P.load_config(commit)


# --- configuration pieces --------------------------------------------------


def component_globs(config: dict) -> tuple:
    """({component: [glob]}, reasons) for every well-formed component."""
    components = config.get("components")
    if not isinstance(components, dict) or not components:
        return {}, [f"components is {brief(components)}, not a mapping of components"]
    globs, reasons = {}, []
    for name, spec in components.items():
        paths = spec.get("paths") if isinstance(spec, dict) else None
        if not isinstance(spec, dict):
            reasons.append(f"components.{name} is not a mapping")
        elif paths is not None and not is_string_list(paths):
            reasons.append(f"components.{name}.paths is not a list of globs")
        else:
            globs[name] = paths or []
    return globs, reasons


def regexes(globs: list, ci: bool = False) -> list:
    return [(P.glob_ci if ci else P.glob_to_regex)(glob) for glob in globs]


def matches(compiled: list, path: str) -> bool:
    return any(regex.fullmatch(path) for regex in compiled)


def gate_list(config: dict) -> tuple:
    """(gates.run as a list, or None, and why it is not one)."""
    gates = config.get("gates")
    if gates is None:
        return [], None
    if not isinstance(gates, dict):
        return None, f"gates is {brief(gates)}, not a mapping"
    run = gates.get("run")
    if run is None:
        return [], None
    if not isinstance(run, list):
        return None, f"gates.run is {brief(run)}, not a list"
    return run, None


def gate_commands(config: dict) -> tuple:
    """([(label, command)] for every gate with a command string, and why gates.run cannot be read)."""
    run, error = gate_list(config)
    if run is None:
        return [], error
    commands = []
    for index, gate in enumerate(run):
        if isinstance(gate, dict) and isinstance(gate.get("command"), str):
            name = gate.get("name")
            commands.append((name if is_text(name) else f"gates.run[{index}]", gate["command"]))
    return commands, None


def runnable_gates(config: dict) -> list:
    """The gates gates-run accepts: mappings with non-empty name and command strings, the first of each name."""
    run, _ = gate_list(config)
    gates, seen = [], set()
    for gate in run or []:
        if isinstance(gate, dict) and is_text(gate.get("name")) and is_text(gate.get("command")):
            if gate["name"] not in seen:
                seen.add(gate["name"])
                gates.append(gate)
    return gates


def gate_words(config: dict) -> list:
    """The plan floor's gate words: shlex words of every gate command, a leading ./ dropped, each also joined
    to the directory of every earlier `cd ARG` in the same command (leading ./, `.` and empty segments dropped)."""
    commands, _ = gate_commands(config)
    words = []
    for _, command in commands:
        try:
            split = shlex.split(command)
        except ValueError:
            continue
        directory = []
        for index, word in enumerate(split):
            stripped = strip_dot(word)
            words.append(stripped)
            if directory:
                words.append("/".join([*directory, stripped]))
            if index > 0 and split[index - 1] == CD:
                directory += [segment for segment in stripped.split("/") if segment not in ("", ".")]
    return words


def require_human(config: dict, section: str, globs: dict) -> tuple:
    """(case-insensitive regexes of pipeline.<section>.require_human, reasons it cannot be read)."""
    where = f"pipeline.{section}.require_human"
    pipeline = config.get("pipeline")
    block = pipeline.get(section) if isinstance(pipeline, dict) else None
    if not isinstance(block, dict):
        return [], [f"pipeline.{section} is {brief(block)}, not a mapping"]
    rh = block.get("require_human")
    if rh is None:
        rh = {}
    if not isinstance(rh, dict):
        return [], [f"{where} is {brief(rh)}, not a mapping"]
    reasons, compiled = [], []
    paths = rh.get("paths")
    if paths is not None and not is_string_list(paths):
        reasons.append(f"{where}.paths is not a list of globs")
    else:
        compiled += regexes(paths or [], ci=True)
    components = rh.get("components")
    if components is not None and not is_string_list(components):
        reasons.append(f"{where}.components is not a list of component names")
    else:
        for name in components or []:
            if name in globs:
                compiled += regexes(globs[name], ci=True)
            else:
                reasons.append(f"{where}.components names {name}, which is not a well-formed component")
    return compiled, reasons


# --- gate commands ---------------------------------------------------------


def simple_commands(command: str) -> list:
    """The simple commands of a gate command, each a list of words; raises CommandError outside the grammar."""
    for character in command:
        if character not in COMMAND_CHARACTERS:
            raise CommandError(f"it holds {character!r}, which the check does not read")
    commands, current, pending = [], [], None  # pending: the last operator, until a word follows it
    for line in command.split("\n"):
        for chunk in line.split(" "):
            for piece in OPERATORS.split(chunk):
                if piece == "" or piece in REDIRECTIONS:
                    continue
                if OPERATORS.fullmatch(piece):
                    if not current:
                        raise CommandError(
                            f"it has {piece!r} without a command before it, which the check does not read"
                        )
                    commands.append(current)
                    current, pending = [], piece
                elif WORD.fullmatch(piece):
                    current.append(piece)
                    pending = None
                else:
                    raise CommandError(f"it holds {piece!r}, which the check does not read")
        if current:  # a newline ends a simple command
            commands.append(current)
            current = []
    if pending in ("&&", "||", "|"):
        raise CommandError(f"it ends with {pending!r}, which the check does not read")
    return commands


def command_word(words: list) -> list:
    """The words from the command word on, empty when there is none; raises CommandError outside the grammar."""
    i = 0
    while i < len(words) and words[i] in RESERVED:
        i += 1
    rest = words[i:]
    if not rest:
        return []
    first = rest[0]
    if "=" in first:
        raise CommandError(f"its command word {first} holds =, which the check does not read")
    if first.startswith("+"):
        raise CommandError(f"its command word {first} starts with +, which the check does not read")
    if first in PREFIXES:
        raise CommandError(f"it runs {first}, a prefix the check does not read")
    if first in INTERPRETERS:
        if len(rest) < 2:
            raise CommandError(f"{first} is given no script, which the check does not read")
        if rest[1].startswith(("-", "+")):
            raise CommandError(f"{first} is given {rest[1]} before its script, an option the check does not read")
    elif first in BUILTINS and first not in ALLOWED_BUILTINS and first != CD:
        raise CommandError(f"it runs {first}, a builtin the check does not read")
    return rest


def cd_chain(command: str) -> list:
    """The arguments of each cd in a command that runs cd; raises CommandError unless it is a pure && chain."""
    if "\n" in command:
        raise CommandError("it runs cd in a command not joined by && alone, which the check does not read")
    simple, current = [], []
    for chunk in command.split(" "):
        if chunk == "":
            continue
        if chunk == "&&":
            simple.append(current)
            current = []
            continue
        if any(OPERATORS.fullmatch(piece) for piece in OPERATORS.split(chunk)):
            raise CommandError("it runs cd in a command not joined by && alone, which the check does not read")
        current.append(chunk)
    simple.append(current)
    arguments = []
    for chunks in simple:
        words = [chunk for chunk in chunks if chunk not in REDIRECTIONS]
        if words and words[0] in RESERVED:
            raise CommandError(f"it runs cd in a command with {words[0]}, which the check does not read")
        if words and words[0] == CD:
            if len(words) != len(chunks):
                raise CommandError("it runs cd with a redirection, which the check does not read")
            if len(words) != 2:
                raise CommandError(
                    f"it runs cd with {len(words) - 1} arguments, but the check reads cd with exactly one"
                )
            arguments.append(words[1])
    return arguments


def cd_directory(argument: str, cwd: str, directories: set) -> str:
    """The root-relative directory `cd argument` leads to from cwd; raises CommandError unless it is tracked."""
    if argument != "." and not argument.startswith("./"):
        raise CommandError(f"it runs cd {argument}, which does not start with ./, so CDPATH can change where it goes")
    segments = argument.split("/")[1:]
    if any(segment in ("", "..") for segment in segments):
        raise CommandError(f"it runs cd {argument}, which has an empty or .. segment, which the check does not read")
    target = join(cwd, "/".join(segment for segment in segments if segment != "."))
    if target not in directories:
        where = f" (as {target})" if cwd else ""
        raise CommandError(f"it runs cd {argument}{where}, which is not a tracked directory")
    return target


def read_gate(command: str, directories: set) -> list:
    """[(working directory, words from the command word on)] for each simple command of a gate command, the
    directory root-relative and empty for the root; raises CommandError outside the grammar."""
    raw = simple_commands(command)
    simple = [command_word(words) for words in raw]
    if not any(words and words[0] == CD for words in simple):
        return [("", words) for words in simple]
    arguments = iter(cd_chain(command))
    cwd, read = "", []
    for words in simple:
        if words and words[0] == CD:
            cwd = cd_directory(next(arguments), cwd, directories)
        read.append((cwd, words))
    return read


def join(directory: str, path: str) -> str:
    return f"{directory}/{path}" if directory and path else directory or path


def resolved(word: str, cwd: str) -> str:
    """The root-relative path a word names from the working directory cwd, a leading ./ dropped."""
    return word if word.startswith("/") else join(cwd, strip_dot(word))


def tracked_problem(word: str, regular: set, blobs: set, cwd: str = "") -> str:
    """Why word, from the working directory cwd, is not a tracked regular file, or empty."""
    path = resolved(word, cwd)
    name = path if cwd else word
    if path in regular:
        return ""
    if path in blobs:
        return f"{name}, which is a tracked symbolic link, not a regular file"
    return f"{name}, which is not a tracked file"


def locked_binary(path: str, regular: set) -> bool:
    """Whether path is [DIR/]node_modules/.bin/NAME under a tracked lockfile in DIR or a directory above it."""
    segments = path.split("/")
    if len(segments) < 3 or segments[-3:-1] != ["node_modules", ".bin"] or segments[-1] in ("", ".", ".."):
        return False
    directory = segments[:-3]
    if any(segment in ("", ".", "..", "node_modules") for segment in directory):
        return False
    return any(
        join("/".join(directory[:depth]), lockfile) in regular
        for depth in range(len(directory), -1, -1)
        for lockfile in LOCKFILES
    )


def script_problem(words: list, regular: set, blobs: set, cwd: str = "") -> str:
    """Why a readable simple command, run in the directory cwd, runs code that no tracked regular file it names
    holds, or empty."""
    if not words:
        return ""
    first = words[0]
    if first == CD:
        return ""
    if first in INTERPRETERS:
        problem = tracked_problem(words[1], regular, blobs, cwd)
        return f"{first} runs {problem}" if problem else ""
    if first in ALLOWED_BUILTINS:
        return ""
    if first in WRAPPERS or (first == "find" and any(w in FIND_EXEC for w in words[1:])):
        return f"it runs {first}, which runs another command the check does not follow"
    if first in TASK_RUNNERS or (len(words) > 1 and words[1] in SUBCOMMAND_RUNNERS.get(first, ())):
        return f"it runs {first}, which runs what an untracked recipe says"
    if first in UNMODELLED or VERSIONED.fullmatch(first):
        return f"it runs {first}, an interpreter the check does not read"
    if "/" in first:
        path = resolved(first, cwd)
        if path not in blobs and locked_binary(path, regular):
            return ""
        problem = tracked_problem(first, regular, blobs, cwd)
        return f"it runs {problem}" if problem else ""
    return ""


# --- signals: tracker, components, review ------------------------------------


def tracker_kinds(ref: Ref) -> tuple:
    tracker = ref.config.get("tracker")
    if not isinstance(tracker, dict):
        return outcome([f"tracker is {brief(tracker)}, not a mapping, so no issue can be read from a tracker"])
    reasons = []
    if not is_text(tracker.get("kind")):
        reasons.append("tracker.kind is not a non-empty string, so no tracker is chosen")
    kinds = tracker.get("kinds")
    if not isinstance(kinds, dict):
        reasons.append("tracker.kinds is not a mapping, so design, plan and implementation issues cannot be told apart")
    else:
        key = KIND_KEYS.get(kinds.get("source")) if isinstance(kinds.get("source"), str) else None
        if key is None:
            reasons.append(
                f"tracker.kinds.source is {brief(kinds.get('source'))}, not issue_types or labels, "
                "so issue kinds cannot be read"
            )
        else:
            for kind in ("design", "plan", "implementation"):
                if not is_text(dig(kinds, kind, key)):
                    reasons.append(f"tracker.kinds.{kind}.{key} is not set, so {kind} issues cannot be recognised")
    components = tracker.get("components")
    source = components.get("source") if isinstance(components, dict) else None
    if not isinstance(components, dict):
        reasons.append("tracker.components is not a mapping, so an issue's component cannot be read")
    elif source == "labels":
        label = components.get("label")
        if not isinstance(label, str) or "{name}" not in label:
            reasons.append("tracker.components.label has no {name}, so component labels cannot be matched")
    elif source != "field":
        reasons.append(
            f"tracker.components.source is {brief(source)}, not labels or field, so an issue's component cannot be read"
        )
    return outcome(reasons)


def trusted_ingest(ref: Ref) -> tuple:
    ingest = dig(ref.config, "tracker", "ingest")
    if not isinstance(ingest, dict):
        return outcome([f"tracker.ingest is {brief(ingest)}, not a mapping, so no issue is trusted to start a job"])
    reasons = []
    trusted = ingest.get("trusted")
    if not isinstance(trusted, dict):
        reasons.append("tracker.ingest.trusted is not a mapping, so no issue is trusted to start a job")
        trusted = {}
    lists = {}
    for key in ("labels", "authors"):
        value = trusted.get(key)
        if value is not None and not is_string_list(value):
            reasons.append(f"tracker.ingest.trusted.{key} is not a list of strings")
        lists[key] = [v for v in value if v] if is_string_list(value) else []
    match = ingest.get("match")
    if match not in ("any", "all"):
        reasons.append(f"tracker.ingest.match is {brief(match)}, not any or all")
    elif not lists["labels"] and not lists["authors"]:
        reasons.append("tracker.ingest.trusted lists no labels and no authors, so no issue is trusted to start a job")
    elif match == "all" and not (lists["labels"] and lists["authors"]):
        empty = "labels" if not lists["labels"] else "authors"
        reasons.append(f"tracker.ingest.match is all but trusted.{empty} is empty, so no issue is trusted")
    return outcome(reasons)


def components_cover(ref: Ref) -> tuple:
    globs, reasons = component_globs(ref.config)
    shared = ref.config.get("shared")
    if shared is not None and not is_string_list(shared):
        reasons.append("shared is not a list of globs")
        shared = []
    compiled = {name: regexes(paths) for name, paths in globs.items()}
    shared_compiled = regexes(shared or [])
    for path in ref.paths:
        owners = [name for name, rx in compiled.items() if matches(rx, path)]
        in_shared = matches(shared_compiled, path)
        if not owners and not in_shared:
            reasons.append(f"{path} is in no component and not in shared, so no issue can change it")
        elif len(owners) > 1 or (owners and in_shared):
            where = ", ".join(owners) + (" and shared" if in_shared else "")
            reasons.append(f"{path} is in more than one of components {where}, so its owner is ambiguous")
    return outcome(reasons)


def codeowners(ref: Ref) -> tuple:
    found = next((path for path in CODEOWNERS if path in ref.by_path), None)
    if found is None:
        return outcome([f"no CODEOWNERS file ({', '.join(CODEOWNERS)}), so no code owner is asked to review"])
    if not ref.regular(found):
        return outcome([f"{found} is not a regular file, so GitHub reads no code owners"])
    for line in ref.blob(found).decode("utf-8", errors="replace").split("\n"):
        words = line.split()
        if not words or words[0].startswith("#"):
            continue
        owners = []
        for word in words[1:]:
            if word.startswith("#"):
                break
            owners.append(word)
        if any(OWNER.fullmatch(word) for word in owners):
            return outcome([])
    return outcome([f"{found} has no rule naming an owner, so no code owner is asked to review"])


def reviewer_logins(value, where: str, reasons: list) -> list:
    if value is None:
        return []
    if not isinstance(value, list):
        reasons.append(f"{where} is not a list of logins")
        return []
    logins = []
    for item in value:
        login = item[1:] if isinstance(item, str) and item.startswith("@") else item
        if not isinstance(login, str) or not LOGIN.fullmatch(login):
            reasons.append(f"{where}: {brief(item)} is not a valid login, so its reviews cannot be requested")
        else:
            logins.append(login)
    return logins


def reviewers(ref: Ref) -> tuple:
    globs, reasons = component_globs(ref.config)
    components = ref.config.get("components")
    per_component = []
    for name, paths in globs.items():
        logins = reviewer_logins(components[name].get("reviewers"), f"components.{name}.reviewers", reasons)
        per_component.append((regexes(paths), logins))
    default = reviewer_logins(dig(ref.config, "pipeline", "review", "reviewers"), "pipeline.review.reviewers", reasons)
    for path in ref.paths:
        found = set()
        for compiled, logins in per_component:
            if matches(compiled, path):
                found.update(logins)
        if not found and not default:
            reasons.append(
                f"{path} has no reviewer: none of its components lists reviewers and pipeline.review.reviewers "
                "is empty, so a PR changing it requests no review"
            )
    return outcome(reasons)


# --- signals: agent instructions ---------------------------------------------


def agents_md(ref: Ref) -> tuple:
    if ref.regular("AGENTS.md"):
        return outcome([])
    return outcome(["AGENTS.md is not a regular file at the ref, so agents get no repository instructions"])


def claude_md(ref: Ref) -> tuple:
    entry = ref.by_path.get("CLAUDE.md")
    if entry is not None and entry[0] == SYMLINK and ref.blob("CLAUDE.md") == b"AGENTS.md":
        return outcome([])
    if entry is not None and entry[0] in REGULAR:
        lines = ref.blob("CLAUDE.md").decode("utf-8", errors="replace").split("\n")
        if any(line.rstrip("\r") == "@AGENTS.md" for line in lines):
            return outcome([])
    return outcome(
        [
            "CLAUDE.md is neither a symbolic link to AGENTS.md nor a file with a line @AGENTS.md, "
            "so Claude Code does not read AGENTS.md"
        ]
    )


def contributing(ref: Ref) -> tuple:
    if ref.regular("CONTRIBUTING.md"):
        return outcome([])
    return outcome(["CONTRIBUTING.md is not a regular file at the ref, so agents get no contribution rules"])


def architecture_docs(ref: Ref) -> tuple:
    if any(path.startswith(ARCHITECTURE) for path in ref.paths):
        return outcome([])
    return outcome([f"nothing is tracked under {ARCHITECTURE}, so agents get no description of the architecture"])


def drift_scope(path: str) -> bool:
    """A path the renderer reads or owns: .mintswarm/stages/*.md or .claude/agents/msw-*.md."""
    if path.startswith(STAGE_SOURCES):
        return fnmatchcase(path[len(STAGE_SOURCES) :], "*.md") and "/" not in path[len(STAGE_SOURCES) :]
    if path.startswith(RENDERED_AGENTS):
        rest = path[len(RENDERED_AGENTS) :]
        return "/" not in rest and fnmatchcase(rest, "msw-*.md")
    return False


def shadowed_directories(ref: Ref) -> list:
    """Directories in the renderer's scope, which it would read as files."""
    found = []
    for path in ref.paths:
        for prefix in (STAGE_SOURCES, RENDERED_AGENTS):
            rest = path[len(prefix) :] if path.startswith(prefix) else ""
            if "/" in rest and drift_scope(prefix + rest.split("/")[0]):
                directory = prefix + rest.split("/")[0]
                if directory not in found:
                    found.append(directory)
    return found


def render_copy(ref: Ref, selected: list) -> list:
    """Run the checker's renderer over a temporary copy of the selected paths; return its reasons."""
    tmp = tempfile.mkdtemp(prefix="msw-readiness-")
    root = Path(tmp)

    def clean(text: str) -> str:
        return " ".join(text.replace(tmp + os.sep, "").replace(tmp, "<copy>").split())

    try:
        (root / STAGE_SOURCES).mkdir(parents=True)
        (root / RENDERED_AGENTS).mkdir(parents=True)
        for path in selected:
            (root / path).write_bytes(ref.blob(path))
        saved = (R.ROOT, R.SOURCES, R.TARGET)
        R.ROOT, R.SOURCES, R.TARGET = root, root / STAGE_SOURCES, root / RENDERED_AGENTS
        out = io.StringIO()
        try:
            with contextlib.redirect_stdout(out):
                drift = R.check(R.load())
        except (R.SourceError, OSError, UnicodeDecodeError) as exc:
            return [f"{clean(str(exc))}, so the stage agents cannot be rendered"]
        finally:
            R.ROOT, R.SOURCES, R.TARGET = saved
    finally:
        shutil.rmtree(tmp)
    lines = sorted(
        clean(line) for line in out.getvalue().split("\n") if line.startswith(("missing: ", "differs: ", "orphan: "))
    )
    if drift and not lines:
        lines = ["render-agents.py reports drift"]
    return lines


def agents_drift(ref: Ref) -> tuple:
    if RENDERER not in ref.by_path:
        return "pass", [f"not applicable: the ref has no {RENDERER}"]
    notes = []
    own = (DOGFOOD / "render-agents.py").read_bytes()
    if not ref.regular(RENDERER) or ref.blob(RENDERER) != own:
        notes.append(f"the ref's {RENDERER} differs from the checker's, which is the one run here")
    selected = [path for path in ref.paths if drift_scope(path)]
    reasons = [
        f"{path} is not a regular file (mode {ref.by_path[path][0]}), so the renderer cannot read it"
        for path in selected
        if not ref.regular(path)
    ]
    reasons += [f"{path} is a directory, so the renderer cannot read it" for path in shadowed_directories(ref)]
    if not reasons:
        reasons = render_copy(ref, selected)
    return ("fail" if reasons else "pass"), reasons + notes


# --- signals: gates ---------------------------------------------------------


def gates_run(ref: Ref) -> tuple:
    run, error = gate_list(ref.config)
    if run is None:
        return outcome([f"{error}, so no gate runs"])
    if not run:
        return outcome(["gates.run is empty, so no change is checked before review"])
    reasons, seen = [], set()
    for index, gate in enumerate(run):
        if not isinstance(gate, dict):
            reasons.append(f"gates.run[{index}] is not a mapping, so it cannot run")
            continue
        name = gate.get("name")
        if not is_text(name):
            reasons.append(f"gates.run[{index}] has no name, so its result cannot be reported")
        elif name in seen:
            reasons.append(f"gates.run[{index}]: the name {name} is used twice, so its results cannot be told apart")
        else:
            seen.add(name)
        if not is_text(gate.get("command")):
            reasons.append(f"gates.run[{index}] has no command, so it cannot run")
    return outcome(reasons)


def gate_scripts_named(ref: Ref) -> tuple:
    commands, error = gate_commands(ref.config)
    if error:
        return outcome([f"{error}, so the gates cannot be read"])
    reasons = []
    for label, command in commands:
        try:
            simple = read_gate(command, ref.directories)
        except CommandError as exc:
            reasons.append(f"gate {label}: {exc}, so the floors cannot see what it runs")
            continue
        for cwd, words in simple:
            problem = script_problem(words, ref.regular_files, ref.blobs, cwd)
            if problem:
                reasons.append(f"gate {label}: {problem}, so the floors cannot protect what it runs")
    return outcome(reasons)


def gate_tools(ref: Ref) -> tuple:
    commands, error = gate_commands(ref.config)
    if error:
        return outcome([f"{error}, so the tools of the gates are unknown"])
    reasons, tools = [], ["bash"]
    for label, command in commands:
        try:
            simple = read_gate(command, ref.directories)
        except CommandError as exc:
            reasons.append(f"gate {label}: {exc}, so its tools are unknown")
            continue
        for _, words in simple:
            if not words:
                continue
            first = words[0]
            if "/" in first or first in ALLOWED_BUILTINS or first in ("source", ".", CD) or first in tools:
                continue
            tools.append(first)
    for tool in tools:
        if shutil.which(tool) is None:
            what = "no gate can run here" if tool == "bash" else "the gates that call it cannot run here"
            reasons.append(f"{tool} is not on PATH, so {what}")
    return outcome(reasons)


def tests_gated(ref: Ref) -> tuple:
    words = set(gate_words(ref.config))
    return outcome(
        [
            f"test {path} is named by no gate command, so the gates never run it"
            for path in ref.tests
            if path not in words
        ]
    )


def report_formats(ref: Ref) -> tuple:
    try:
        RG.resolve(ref.config)
    except RG.ConfigError as exc:
        return outcome([f"{exc}, so the runner stops every job before its gates"])
    return outcome([])


# --- signals: plan approval and landing ----------------------------------------


def plan_approval_config(ref: Ref) -> tuple:
    components = ref.config.get("components")
    if not isinstance(components, dict) or not components:
        return outcome([f"components is {brief(components)}, not a mapping of components, so plan approval cannot run"])
    try:
        P.validate_config(ref.config, min(components))
    except P.InputError as exc:
        return outcome([f"{exc}, so the plan floor cannot run"])
    return outcome([])


def docs_component(ref: Ref) -> tuple:
    paths = dig(ref.config, "components", DOCS, "paths")
    if is_string_list(paths) and paths:
        return outcome([])
    return outcome(
        [f"components.{DOCS} has no paths, so the plan floor's tests-none rule has no component that needs no tests"]
    )


def tests_exist(ref: Ref) -> tuple:
    if ref.tests:
        return outcome([])
    return outcome(
        [
            "no test file is tracked (*selftest*, test_*.py, *_test.*, *.test.*, *.spec.* or under tests/), "
            "so no change is verified by tests"
        ]
    )


def tests_declared(ref: Ref) -> tuple:
    gates = ref.config.get("gates")
    tests = gates.get("tests") if isinstance(gates, dict) else None
    if gates is not None and not isinstance(gates, dict):
        return outcome([f"gates is {brief(gates)}, not a mapping, so gates.tests cannot be read"])
    if tests is not None and not isinstance(tests, str):
        return outcome([f"gates.tests is {brief(tests)}, not a string"])
    if tests == "none" and ref.tests:
        count = len(ref.tests)
        files = "1 test file exists" if count == 1 else f"{count} test files exist"
        return outcome([f"gates.tests: none while {files}, so every plan outside the {DOCS} component goes to a human"])
    return outcome([])


def uncovered(paths: list, compiled: list) -> list:
    seen, found = set(), []
    for path in paths:
        if path not in seen:
            seen.add(path)
            if not matches(compiled, path):
                found.append(path)
    return found


def require_human_plan(ref: Ref) -> tuple:
    globs, _ = component_globs(ref.config)
    compiled, reasons = require_human(ref.config, "plan_approval", globs)
    named = [word for word in gate_words(ref.config) if word in ref.blobs]
    protected = [CONFIG, *named, *ref.instructions()]
    reasons += [
        f"{path} is not covered by pipeline.plan_approval.require_human, "
        "so a plan that changes it can be approved without a human"
        for path in uncovered(protected, compiled)
    ]
    return outcome(reasons)


def assess_model(ref: Ref) -> tuple:
    assess = D.expected_judge(ref.config)["model"]
    agents = ref.config.get("agents")
    agents = agents if isinstance(agents, dict) else {}
    default = agents.get("default") if isinstance(agents.get("default"), dict) else {}
    plan = dig(agents, "stages", "plan")
    plan = plan if isinstance(plan, dict) else {}
    model = plan.get("model", default.get("model"))
    reasons = []
    if not isinstance(assess, str):
        reasons.append("no model is set for the assess stage (agents.stages.assess or agents.default)")
    if not isinstance(model, str):
        reasons.append("no model is set for the plan stage (agents.stages.plan or agents.default)")
    if not reasons and assess == model:
        reasons.append(
            f"the assess and plan stages both run on {model}, so a plan is judged by the model that wrote it"
        )
    return outcome(reasons)


def landing_config(ref: Ref) -> tuple:
    components = ref.config.get("components")
    try:
        L.validate_landing(ref.config, components if isinstance(components, dict) else {})
    except (L.InputError, P.InputError) as exc:
        return outcome([f"{exc}, so the landing floor cannot run"])
    return outcome([])


def require_human_landing(ref: Ref) -> tuple:
    globs, _ = component_globs(ref.config)
    compiled, reasons = require_human(ref.config, "landing", globs)
    reasons += [
        f"{path} is not covered by pipeline.landing.require_human, so a change to it can land without a human"
        for path in uncovered(ref.instructions(), compiled)
    ]
    return outcome(reasons)


CHECKS = {
    "tracker-kinds": tracker_kinds,
    "trusted-ingest": trusted_ingest,
    "components-cover": components_cover,
    "codeowners": codeowners,
    "reviewers": reviewers,
    "agents-md": agents_md,
    "claude-md": claude_md,
    "contributing": contributing,
    "architecture-docs": architecture_docs,
    "agents-drift": agents_drift,
    "gates-run": gates_run,
    "gate-scripts-named": gate_scripts_named,
    "gate-tools": gate_tools,
    "tests-gated": tests_gated,
    "report-formats": report_formats,
    "plan-approval-config": plan_approval_config,
    "docs-component": docs_component,
    "tests-exist": tests_exist,
    "tests-declared": tests_declared,
    "require-human-plan": require_human_plan,
    "assess-model": assess_model,
    "landing-config": landing_config,
    "require-human-landing": require_human_landing,
}


def signal_entry(feature: str, name: str, result: str, reasons: list) -> dict:
    return {"feature": feature, "signal": name, "result": result, "reasons": reasons}


def evaluate(ref: Ref) -> list:
    signals = []
    for feature, name, reads_config in SIGNALS:
        if reads_config and ref.config is None:
            result, reasons = "fail", [f"no {CONFIG} at {ref.commit}"]
        else:
            result, reasons = CHECKS[name](ref)
        signals.append(signal_entry(feature, name, result, reasons))
    return signals


# --- running the gates ------------------------------------------------------


class Terminate:
    """SIGTERM while the gates run: raise at once, or after a cleanup in progress."""

    def __init__(self):
        self.deferred = False
        self.pending = False

    def __call__(self, signum, frame):
        self.pending = True
        if not self.deferred:
            raise InputError("terminated by SIGTERM while running the gates")

    @contextlib.contextmanager
    def held(self):
        self.deferred = True
        try:
            yield
        finally:
            self.deferred = False
        if self.pending:
            raise InputError("terminated by SIGTERM while running the gates")


def gate_environment(env: dict) -> dict:
    """The starting environment without git's repository-local variables."""
    return {k: v for k, v in env.items() if k not in GIT_LOCAL_ENV and not k.startswith(GIT_LOCAL_PREFIXES)}


def worktree_environment() -> dict:
    return {k: v for k, v in os.environ.items() if k not in WORKTREE_DROPPED_ENV}


def restore_writable(path: str) -> None:
    """Give the owner read, write and search permission on every directory under path, symbolic links not followed."""
    try:
        info = os.lstat(path)
    except OSError:
        return
    if not stat.S_ISDIR(info.st_mode):
        return
    try:
        os.chmod(path, stat.S_IMODE(info.st_mode) | stat.S_IRWXU)
        names = os.listdir(path)
    except OSError:
        return
    for name in names:
        restore_writable(os.path.join(path, name))


def run_in_group(argv: list, timeout: int, **kwargs) -> tuple:
    """Run argv in a new session; return (returncode or None, timed out). Its process group is always killed."""
    proc = subprocess.Popen(argv, start_new_session=True, **kwargs)
    try:
        try:
            return proc.wait(timeout=timeout), False
        except subprocess.TimeoutExpired:
            return None, True
    finally:
        G.kill_group(proc)  # also what the gate left running in the background
        proc.wait()


def remove_worktree(name: str, parent, worktree: str, terminate: Terminate) -> None:
    with terminate.held():
        problems = []
        if parent is not None:
            restore_writable(parent)
            if os.path.lexists(worktree):
                proc = subprocess.run(
                    ["git", "worktree", "remove", "--force", "--force", worktree],
                    capture_output=True,
                    env=worktree_environment(),
                )
                if proc.returncode != 0:
                    detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
                    problems.append(f"git worktree remove failed: {detail[-1] if detail else proc.returncode}")
            try:
                shutil.rmtree(parent)
            except OSError as exc:
                problems.append(f"cannot remove its temporary directory: {exc.strerror}")
        if problems:
            raise InputError(f"gate {name}: cannot clean up its worktree: {'; '.join(problems)}")


def run_gate(gate: dict, commit: str, env: dict, terminate: Terminate) -> dict:
    name = gate["name"]
    timeout = gate.get("timeout")
    if not isinstance(timeout, int) or isinstance(timeout, bool) or timeout <= 0:
        timeout = G.DEFAULT_TIMEOUT_S
    parent = None
    worktree = ""
    try:
        parent = tempfile.mkdtemp(prefix="msw-readiness-gate-")
        worktree = os.path.join(parent, "wt")
        proc = subprocess.run(
            ["git", "-c", "core.hooksPath=/dev/null", "worktree", "add", "--detach", worktree, commit],
            capture_output=True,
            env=worktree_environment(),
        )
        if proc.returncode != 0:
            detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
            raise InputError(f"gate {name}: cannot add a worktree: {detail[-1] if detail else proc.returncode}")
        try:
            code, timed_out = run_in_group(
                ["bash", "-c", gate["command"]],
                timeout,
                cwd=worktree,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except OSError as exc:
            return signal_entry("gates", f"gate:{name}", "fail", [f"bash cannot start: {exc.strerror}"])
    finally:
        remove_worktree(name, parent, worktree, terminate)
    if timed_out:
        return signal_entry("gates", f"gate:{name}", "fail", [f"timed out after {timeout} s"])
    if code != 0:
        return signal_entry("gates", f"gate:{name}", "fail", [f"exit code {code}"])
    return signal_entry("gates", f"gate:{name}", "pass", [])


def run_gates(ref: Ref, env: dict) -> list:
    if ref.config is None:
        return [signal_entry("gates", "gate-run", "fail", [f"no {CONFIG} at {ref.commit}"])]
    gates = runnable_gates(ref.config)
    if not gates:
        return [signal_entry("gates", "gate-run", "fail", ["gates.run has no gate that can run"])]
    terminate = Terminate()
    previous = signal.signal(signal.SIGTERM, terminate)
    try:
        return [run_gate(gate, ref.commit, gate_environment(env), terminate) for gate in gates]
    finally:
        signal.signal(signal.SIGTERM, previous)


# --- verdict ----------------------------------------------------------------


def write_verdict(path: str, verdict: dict) -> None:
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".readiness.", suffix=".json", dir=target.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            out.write(json.dumps(verdict, indent=2, ensure_ascii=False) + "\n")
        os.replace(tmp, target)
    except BaseException:
        try:
            remove(tmp)
        except OSError:
            pass
        raise


def readiness(argv: list) -> int:
    remove_stale(argv)
    args = parse_args(argv)
    env = dict(os.environ)  # the gates get the environment the check was started with
    load_helpers(args["run_gates"])
    L.neutral_git_config()
    commit = resolve(args["ref"])
    ref = Ref(commit, ls_tree(commit), None)
    ref.config = read_config(commit, ref.by_path)
    signals = evaluate(ref)
    if args["run_gates"]:
        signals += run_gates(ref, env)
    gaps = any(s["result"] == "fail" for s in signals)
    verdict = {
        "tool": TOOL,
        "version": VERSION,
        "ref": {"ref": args["ref"], "commit": commit},
        "config": CONFIG if ref.config is not None else None,
        "verdict": "gaps" if gaps else "ready",
        "signals": signals,
    }
    if args["json"] is not None:
        write_verdict(args["json"], verdict)
    lines = []
    for s in signals:
        line = f"{s['result']:7} {s['feature']} {s['signal']}"
        if s["reasons"]:
            line += f": {L.printable(s['reasons'][0])}"
        lines.append(line)
    lines.append(f"readiness: {verdict['verdict']}")
    print("\n".join(lines))
    return 1 if gaps else 0


def is_input_error(exc: Exception) -> bool:
    if isinstance(exc, InputError):
        return True
    return any(module is not None and isinstance(exc, module.InputError) for module in (P, L))


def main(argv: list) -> int:
    try:
        return readiness(argv)
    except Exception as exc:  # a crash must never read as ready or as gaps
        message = " ".join(str(exc).split())
        if is_input_error(exc):
            print(f"readiness: {message}", file=sys.stderr)
            return 2
        for path in stale_targets(argv):
            try:
                remove(path)
            except Exception:  # the handler itself must always end in exit 2
                pass
        print(f"readiness: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
