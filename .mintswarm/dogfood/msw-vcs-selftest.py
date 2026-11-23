#!/usr/bin/env python3
"""Self-test for the msw-vcs shim, run by the msw-vcs-selftest gate.

    msw-vcs-selftest.py

Builds a fixture repository in a temporary directory, with pinned identities
and dates, and runs a table of cases against .mintswarm/dogfood/msw-vcs. A
case is an argv with the exact stdout, stderr and exit code the contract in
.mintswarm/dogfood/msw-vcs.md requires, and the job input vcs.json it runs
with. Commit and object ids are written as `{=name}` placeholders, filled in
from the fixture once it is built. The cases are data, so the msw-vcs tool
can replay them unchanged. One further check, which only the shim has,
compares its diff options with the landing floor's DIFF_OPTIONS.

The fixture's history, with F the merge base:

    F   the root: README.md, keep.txt, mode.sh, gone.txt, kind, docs/guide.md
    B   master, the job's base, child of F: adds main.txt
    J1  job, child of F: modifies README.md, deletes gone.txt, adds bin.dat,
        makes mode.sh executable
    J2  job, the head, child of J1: adds a symlink, a gitlink and paths that
        need quoting, and turns kind into a symlink

with a branch `topic`, a tag `v1`, a remote ref `origin/master`, a dangling
commit D and an orphan commit O that shares no history. Prints one line per
case; exits 0 only when every case passes, 1 otherwise. Leaves no file in the
repository. Standard library only.
"""

import ast
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOL = HERE / "msw-vcs"
FLOOR = HERE / "landing" / "floor.py"
GITLINK_ID = "0123456789abcdef0123456789abcdef01234567"
PLACEHOLDER = re.compile(r"\{=([A-Za-z0-9_]+)\}")
DEFAULT_VCS_JSON = '{"base": "{=B}"}'
QUOTED_NAME = 'we"ird\tname.txt'
UMLAUT_NAME = "café.txt"
USAGE = (
    "usage: msw-vcs log [REV | REV..REV] [-- PATH...]"
    " | msw-vcs diff [--numstat | --name-only | --raw] (REV REV | --job) [-- PATH...]"
    " | msw-vcs show REV[:PATH]"
)


def run(argv, cwd, env, data=None) -> bytes:
    proc = subprocess.run(argv, cwd=cwd, env=env, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if proc.returncode != 0:
        raise RuntimeError(f"{argv!r} failed: {proc.stderr.decode(errors='replace')}")
    return proc.stdout


class Fixture:
    def __init__(self, tmp: Path):
        self.repo = tmp / "repo"
        home = tmp / "home"
        self.repo.mkdir()
        home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("GIT_", "MSW_", "LC_", "LANG"))}
        env.update(
            {
                "HOME": str(home),
                "TMPDIR": str(tmp),
                "LC_ALL": "C",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_TERMINAL_PROMPT": "0",
                "GIT_AUTHOR_NAME": "Ada Author",
                "GIT_AUTHOR_EMAIL": "ada@example.invalid",
                "GIT_COMMITTER_NAME": "Cy Committer",
                "GIT_COMMITTER_EMAIL": "cy@example.invalid",
            }
        )
        self.env = env
        self.ids = {}
        self.git("init", "-q", "--object-format=sha1", "-b", "master")

        self.write("README.md", "readme\nline two\n")
        self.write("keep.txt", "kept\n")
        self.write("mode.sh", "echo mode\n")
        self.write("gone.txt", "to be deleted\n")
        self.write("kind", "a regular file\n")
        self.write("docs/guide.md", "guide\n")
        files = ("README.md", "keep.txt", "mode.sh", "gone.txt", "kind", "docs/guide.md")
        self.commit("F", "2026-01-01T00:00:00Z", "start", *files)

        self.git("checkout", "-q", "-b", "job")
        self.git("checkout", "-q", "master")
        self.write("main.txt", "main line\n")
        self.commit("B", "2026-01-02T00:00:00Z", "main: add main.txt", "main.txt")

        self.git("checkout", "-q", "job")
        self.write("README.md", "readme\nline 2\nline three\n")
        self.git("rm", "-q", "gone.txt")
        (self.repo / "bin.dat").write_bytes(b"\x00\x01\x02binary\x00\n")
        os.chmod(self.repo / "mode.sh", 0o755)
        self.git("update-index", "--chmod=+x", "mode.sh")
        self.commit("J1", "2026-01-03T00:00:00Z", "job: edit the readme", "README.md", "bin.dat", "mode.sh")

        os.symlink("README.md", self.repo / "link")
        (self.repo / "kind").unlink()
        os.symlink("keep.txt", self.repo / "kind")
        self.write(QUOTED_NAME, "quoted\n")
        self.write(UMLAUT_NAME, "accent\n")
        self.git("update-index", "--add", "--cacheinfo", f"160000,{GITLINK_ID},sub")
        self.commit(
            "J2",
            "2026-01-04T12:30:00+02:00",
            "job: add links and odd names\n\nThe body explains.\n\n  An indented line.\n",
            "link",
            "kind",
            QUOTED_NAME,
            UMLAUT_NAME,
        )

        # Two commits after base on master, for the hostile-config cases: a change with a blank context line.
        self.git("checkout", "-q", "master")
        self.write("blank.txt", "first\n\nsecond\n")
        self.commit("X1", "2026-01-07T00:00:00Z", "main: add blank.txt", "blank.txt")
        self.write("blank.txt", "first\n\nthird\n")
        self.commit("X2", "2026-01-08T00:00:00Z", "main: edit blank.txt", "blank.txt")
        self.git("checkout", "-q", "job")

        self.git("branch", "topic", self.ids["B"])
        self.git("tag", "v1", self.ids["F"])
        self.git("update-ref", "refs/remotes/origin/master", self.ids["B"])
        tree = self.rev(f"{self.ids['F']}^{{tree}}")
        self.ids["D"] = self.git_env("2026-01-05T00:00:00Z", "commit-tree", tree, "-p", self.ids["F"], "-m", "dangling")
        for name, expr in {
            "readme0": "F:README.md",
            "readme1": "J1:README.md",
            "keep": "F:keep.txt",
            "mode": "F:mode.sh",
            "gone": "F:gone.txt",
            "kind0": "F:kind",
            "kind1": "J2:kind",
            "guide": "F:docs/guide.md",
            "main": "B:main.txt",
            "blank1": "X1:blank.txt",
            "blank2": "X2:blank.txt",
            "bin": "J1:bin.dat",
            "link": "J2:link",
            "quoted": f"J2:{QUOTED_NAME}",
            "umlaut": f"J2:{UMLAUT_NAME}",
        }.items():
            commit, _, path = expr.partition(":")
            self.ids[name] = self.rev(f"{self.ids[commit]}:{path}")
        self.ids["gitlink"] = GITLINK_ID
        self.ids["O"] = self.git_env("2026-01-06T00:00:00Z", "commit-tree", tree, "-m", "orphan")
        self.ids["Ftree"] = tree
        self.ids["F7"] = self.ids["F"][:7]
        self.ids["F_upper"] = self.ids["F"].upper()
        self.ids["zero"] = "0" * 40
        self.inputs = self.repo / ".mintswarm" / "job" / "inputs"
        self.inputs.mkdir(parents=True)
        self.attributes = self.repo / ".git" / "info" / "attributes"

    def write(self, path: str, text: str) -> None:
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(text.encode("utf-8"))

    def git(self, *args) -> str:
        return run(["git", *args], self.repo, self.env).decode("utf-8").strip()

    def git_env(self, date: str, *args) -> str:
        env = dict(self.env, GIT_AUTHOR_DATE=date, GIT_COMMITTER_DATE=date)
        return run(["git", *args], self.repo, env).decode("utf-8").strip()

    def rev(self, expr: str) -> str:
        return self.git("rev-parse", "--verify", expr)

    def commit(self, name: str, date: str, message: str, *paths) -> None:
        self.git("add", "--", *paths)
        self.git_env(date, "commit", "-q", "-m", message)
        self.ids[name] = self.rev("HEAD")

    def fill(self, text: str) -> str:
        return PLACEHOLDER.sub(lambda m: self.ids[m.group(1)], text)

    def arrange(self, case: dict) -> None:
        """Apply a case's repository attributes file and local configuration."""
        if "attributes" in case:
            self.attributes.parent.mkdir(exist_ok=True)
            self.attributes.write_text(case["attributes"], encoding="utf-8")
        for key, value in case.get("config", {}).items():
            self.git("config", "--local", key, value)

    def restore(self, case: dict) -> None:
        self.attributes.unlink(missing_ok=True)
        for key in case.get("config", {}):
            self.git("config", "--local", "--unset-all", key)

    def tool(self, argv: list, vcs_json) -> tuple:
        target = self.inputs / "vcs.json"
        if vcs_json is None:
            target.unlink(missing_ok=True)
        else:
            target.write_text(self.fill(vcs_json), encoding="utf-8")
        env = dict(self.env, PYTHONDONTWRITEBYTECODE="1")
        proc = subprocess.run(
            [sys.executable, str(TOOL), *[os.fsencode(self.fill(arg)) for arg in argv]],
            cwd=self.repo,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        return proc.stdout, proc.stderr, proc.returncode


CASES = [
    {
        "name": "log: the job's commits, merge-base..head",
        "argv": ["log"],
        "stdout": (
            "commit {=J2}\n"
            "parent {=J1}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-04T10:30:00Z\n"
            "subject job: add links and odd names\n"
            "path A café.txt\n"
            "path T kind\n"
            "path A link\n"
            "path A sub\n"
            'path A "we\\"ird\\tname.txt"\n'
            "\n"
            "commit {=J1}\n"
            "parent {=F}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-03T00:00:00Z\n"
            "subject job: edit the readme\n"
            "path M README.md\n"
            "path A bin.dat\n"
            "path D gone.txt\n"
            "path M mode.sh\n"
        ),
    },
    {
        "name": "log: a revision and its ancestors, a root commit against the empty tree",
        "argv": ["log", "base"],
        "stdout": (
            "commit {=B}\n"
            "parent {=F}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-02T00:00:00Z\n"
            "subject main: add main.txt\n"
            "path A main.txt\n"
            "\n"
            "commit {=F}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-01T00:00:00Z\n"
            "subject start\n"
            "path A README.md\n"
            "path A docs/guide.md\n"
            "path A gone.txt\n"
            "path A keep.txt\n"
            "path A kind\n"
            "path A mode.sh\n"
        ),
    },
    {
        "name": "log: a range of full ids",
        "argv": ["log", "{=F}..{=J1}"],
        "stdout": (
            "commit {=J1}\n"
            "parent {=F}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-03T00:00:00Z\n"
            "subject job: edit the readme\n"
            "path M README.md\n"
            "path A bin.dat\n"
            "path D gone.txt\n"
            "path M mode.sh\n"
        ),
    },
    {
        "name": "log: paths filter the commits and their paths",
        "argv": ["log", "merge-base..head", "--", "kind", "docs"],
        "stdout": (
            "commit {=J2}\n"
            "parent {=J1}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-04T10:30:00Z\n"
            "subject job: add links and odd names\n"
            "path T kind\n"
        ),
    },
    {
        "name": "log: a path the range never changes",
        "argv": ["log", "--", "docs"],
    },
    {
        "name": "log: a range from head to base, the main line's commit",
        "argv": ["log", "head..base"],
        "stdout": (
            "commit {=B}\n"
            "parent {=F}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-02T00:00:00Z\n"
            "subject main: add main.txt\n"
            "path A main.txt\n"
        ),
    },
    {
        "name": "log: an empty range",
        "argv": ["log", "head..head"],
    },
    {
        "name": "show: a commit with its message",
        "argv": ["show", "head"],
        "stdout": (
            "commit {=J2}\n"
            "parent {=J1}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-04T10:30:00Z\n"
            "subject job: add links and odd names\n"
            "path A café.txt\n"
            "path T kind\n"
            "path A link\n"
            "path A sub\n"
            'path A "we\\"ird\\tname.txt"\n'
            "\n"
            "    job: add links and odd names\n"
            "\n"
            "    The body explains.\n"
            "\n"
            "      An indented line.\n"
        ),
    },
    {
        "name": "show: a root commit by full id",
        "argv": ["show", "{=F}"],
        "stdout": (
            "commit {=F}\n"
            "author Ada Author <ada@example.invalid>\n"
            "date 2026-01-01T00:00:00Z\n"
            "subject start\n"
            "path A README.md\n"
            "path A docs/guide.md\n"
            "path A gone.txt\n"
            "path A keep.txt\n"
            "path A kind\n"
            "path A mode.sh\n"
            "\n"
            "    start\n"
        ),
    },
    {
        "name": "show: a file's bytes",
        "argv": ["show", "base:main.txt"],
        "stdout": "main line\n",
    },
    {
        "name": "show: a symlink's target",
        "argv": ["show", "head:link"],
        "stdout": "README.md",
    },
    {
        "name": "show: the whole tree",
        "argv": ["show", "head:"],
        "stdout": (
            "100644 {=readme1}\tREADME.md\n"
            "100644 {=bin}\tbin.dat\n"
            "100644 {=umlaut}\tcafé.txt\n"
            "100644 {=guide}\tdocs/guide.md\n"
            "100644 {=keep}\tkeep.txt\n"
            "120000 {=kind1}\tkind\n"
            "120000 {=link}\tlink\n"
            "100755 {=mode}\tmode.sh\n"
            "160000 {=gitlink}\tsub\n"
            '100644 {=quoted}\t"we\\"ird\\tname.txt"\n'
        ),
    },
    {
        "name": "show: a directory",
        "argv": ["show", "{=J1}:docs"],
        "stdout": "100644 {=guide}\tdocs/guide.md\n",
    },
    {
        "name": "show: a directory at head",
        "argv": ["show", "head:docs"],
        "stdout": "100644 {=guide}\tdocs/guide.md\n",
    },
    {
        "name": "show: a gitlink",
        "argv": ["show", "head:sub"],
        "stdout": "160000 {=gitlink}\tsub\n",
    },
    {
        "name": "diff: the job's patch",
        "argv": ["diff", "--job"],
        "stdout": (
            "diff --git a/README.md b/README.md\n"
            "index {=readme0}..{=readme1} 100644\n"
            "--- a/README.md\n"
            "+++ b/README.md\n"
            "@@ -1,2 +1,3 @@\n"
            " readme\n"
            "-line two\n"
            "+line 2\n"
            "+line three\n"
            "diff --git a/bin.dat b/bin.dat\n"
            "new file mode 100644\n"
            "index {=zero}..{=bin}\n"
            "Binary files /dev/null and b/bin.dat differ\n"
            "diff --git a/café.txt b/café.txt\n"
            "new file mode 100644\n"
            "index {=zero}..{=umlaut}\n"
            "--- /dev/null\n"
            "+++ b/café.txt\n"
            "@@ -0,0 +1 @@\n"
            "+accent\n"
            "diff --git a/gone.txt b/gone.txt\n"
            "deleted file mode 100644\n"
            "index {=gone}..{=zero}\n"
            "--- a/gone.txt\n"
            "+++ /dev/null\n"
            "@@ -1 +0,0 @@\n"
            "-to be deleted\n"
            "diff --git a/kind b/kind\n"
            "deleted file mode 100644\n"
            "index {=kind0}..{=zero}\n"
            "--- a/kind\n"
            "+++ /dev/null\n"
            "@@ -1 +0,0 @@\n"
            "-a regular file\n"
            "diff --git a/kind b/kind\n"
            "new file mode 120000\n"
            "index {=zero}..{=kind1}\n"
            "--- /dev/null\n"
            "+++ b/kind\n"
            "@@ -0,0 +1 @@\n"
            "+keep.txt\n"
            "\\ No newline at end of file\n"
            "diff --git a/link b/link\n"
            "new file mode 120000\n"
            "index {=zero}..{=link}\n"
            "--- /dev/null\n"
            "+++ b/link\n"
            "@@ -0,0 +1 @@\n"
            "+README.md\n"
            "\\ No newline at end of file\n"
            "diff --git a/mode.sh b/mode.sh\n"
            "old mode 100644\n"
            "new mode 100755\n"
            "diff --git a/sub b/sub\n"
            "new file mode 160000\n"
            "index {=zero}..{=gitlink}\n"
            "--- /dev/null\n"
            "+++ b/sub\n"
            "@@ -0,0 +1 @@\n"
            "+Subproject commit {=gitlink}\n"
            'diff --git "a/we\\"ird\\tname.txt" "b/we\\"ird\\tname.txt"\n'
            "new file mode 100644\n"
            "index {=zero}..{=quoted}\n"
            "--- /dev/null\n"
            '+++ "b/we\\"ird\\tname.txt"\n'
            "@@ -0,0 +1 @@\n"
            "+quoted\n"
        ),
    },
    {
        "name": "diff: the job's patch for three paths",
        "argv": ["diff", "--job", "--", "README.md", "bin.dat", "mode.sh"],
        "stdout": (
            "diff --git a/README.md b/README.md\n"
            "index {=readme0}..{=readme1} 100644\n"
            "--- a/README.md\n"
            "+++ b/README.md\n"
            "@@ -1,2 +1,3 @@\n"
            " readme\n"
            "-line two\n"
            "+line 2\n"
            "+line three\n"
            "diff --git a/bin.dat b/bin.dat\n"
            "new file mode 100644\n"
            "index {=zero}..{=bin}\n"
            "Binary files /dev/null and b/bin.dat differ\n"
            "diff --git a/mode.sh b/mode.sh\n"
            "old mode 100644\n"
            "new mode 100755\n"
        ),
    },
    {
        "name": "diff: --numstat",
        "argv": ["diff", "--numstat", "--job"],
        "stdout": (
            "2\t1\tREADME.md\n"
            "-\t-\tbin.dat\n"
            "1\t0\tcafé.txt\n"
            "0\t1\tgone.txt\n"
            "1\t1\tkind\n"
            "1\t0\tlink\n"
            "0\t0\tmode.sh\n"
            "1\t0\tsub\n"
            '1\t0\t"we\\"ird\\tname.txt"\n'
        ),
    },
    {
        "name": "diff: --name-only",
        "argv": ["diff", "--name-only", "--job"],
        "stdout": (
            "README.md\n"
            "bin.dat\n"
            "café.txt\n"
            "gone.txt\n"
            "kind\n"
            "link\n"
            "mode.sh\n"
            "sub\n"
            '"we\\"ird\\tname.txt"\n'
        ),
    },
    {
        "name": "diff: --raw",
        "argv": ["diff", "--raw", "--job"],
        "stdout": (
            ":100644 100644 {=readme0} {=readme1} M\tREADME.md\n"
            ":000000 100644 {=zero} {=bin} A\tbin.dat\n"
            ":000000 100644 {=zero} {=umlaut} A\tcafé.txt\n"
            ":100644 000000 {=gone} {=zero} D\tgone.txt\n"
            ":100644 120000 {=kind0} {=kind1} T\tkind\n"
            ":000000 120000 {=zero} {=link} A\tlink\n"
            ":100644 100755 {=mode} {=mode} M\tmode.sh\n"
            ":000000 160000 {=zero} {=gitlink} A\tsub\n"
            ':000000 100644 {=zero} {=quoted} A\t"we\\"ird\\tname.txt"\n'
        ),
    },
    {
        "name": "diff: two revisions with a path",
        "argv": ["diff", "merge-base", "base", "--", "main.txt"],
        "stdout": (
            "diff --git a/main.txt b/main.txt\n"
            "new file mode 100644\n"
            "index {=zero}..{=main}\n"
            "--- /dev/null\n"
            "+++ b/main.txt\n"
            "@@ -0,0 +1 @@\n"
            "+main line\n"
        ),
    },
    {
        "name": "diff: two revisions, two paths, one unchanged",
        "argv": ["diff", "--name-only", "{=F}", "head", "--", "docs", "kind"],
        "stdout": "kind\n",
    },
    {
        "name": "diff: paths are literal, so glob magic matches nothing",
        "argv": ["diff", "--job", "--", ":(glob)*"],
    },
    {
        "name": "diff: --job and --numstat in either order",
        "argv": ["diff", "--job", "--numstat", "--", "README.md"],
        "stdout": "2\t1\tREADME.md\n",
    },
    {
        "name": "diff: an empty repository attributes file is allowed",
        "argv": ["diff", "--numstat", "--job", "--", "README.md", "bin.dat"],
        "attributes": "",
        "stdout": "2\t1\tREADME.md\n-\t-\tbin.dat\n",
    },
    {
        "name": "diff: hostile local configuration leaves the patch unchanged",
        "argv": ["diff", "{=X1}", "base"],
        "vcs_json": '{"base": "{=X2}"}',
        "config": {"diff.suppressBlankEmpty": "true", "core.bigFileThreshold": "1"},
        "stdout": (
            "diff --git a/blank.txt b/blank.txt\n"
            "index {=blank1}..{=blank2} 100644\n"
            "--- a/blank.txt\n"
            "+++ b/blank.txt\n"
            "@@ -1,3 +1,3 @@\n"
            " first\n"
            " \n"
            "-second\n"
            "+third\n"
        ),
    },
    {
        "name": "diff: hostile local configuration leaves --numstat unchanged",
        "argv": ["diff", "--numstat", "--job", "--", "README.md", "bin.dat"],
        "config": {"diff.suppressBlankEmpty": "true", "core.bigFileThreshold": "1"},
        "stdout": "2\t1\tREADME.md\n-\t-\tbin.dat\n",
    },
]

# The error cases: name, argv, exit code, the stderr message after `msw-vcs: `, the vcs.json when it is not
# the default, and the repository attributes file when there is one.
ERRORS = [
    ("refused: a branch", ["log", "topic"], 2, "refused revision: topic"),
    ("refused: a tag", ["show", "v1"], 2, "refused revision: v1"),
    ("refused: a remote ref", ["log", "origin/master"], 2, "refused revision: origin/master"),
    (
        "refused: a full ref name",
        ["log", "refs/remotes/origin/master"],
        2,
        "refused revision: refs/remotes/origin/master",
    ),
    ("refused: HEAD", ["show", "HEAD"], 2, "refused revision: HEAD"),
    ("refused: an ancestor suffix", ["log", "base~1"], 2, "refused revision: base~1"),
    ("refused: a parent suffix", ["show", "base^"], 2, "refused revision: base^"),
    ("refused: a reflog entry", ["log", "@{1}"], 2, "refused revision: @{1}"),
    ("refused: an abbreviated id", ["show", "{=F7}"], 2, "refused revision: {=F7}"),
    ("refused: an upper-case id", ["show", "{=F_upper}"], 2, "refused revision: {=F_upper}"),
    ("refused: a message search", ["show", ":/start"], 2, "refused revision: :/start"),
    ("refused: a symmetric range", ["log", "merge-base...head"], 2, "refused revision: merge-base...head"),
    ("refused: a range with an empty side", ["log", "base.."], 2, "refused revision: base.."),
    ("refused: a range with an empty left side", ["log", "..head"], 2, "refused revision: ..head"),
    ("refused: a range in diff", ["diff", "base..head"], 2, "refused revision: base..head"),
    ("refused: a range in show", ["show", "base..head"], 2, "refused revision: base..head"),
    ("refused: an option in a revision position", ["log", "--all"], 2, "refused revision: --all"),
    ("refused: an option after a revision", ["diff", "base", "--output=x", "head"], 2, "refused revision: --output=x"),
    ("refused: before the job input is read", ["log", "HEAD"], 2, "refused revision: HEAD", None),
    ("refused: a parent path component", ["diff", "--job", "--", "docs/../kind"], 2, "refused path: docs/../kind"),
    ("refused: a bare parent path", ["log", "--", ".."], 2, "refused path: .."),
    ("refused: a current-directory path", ["diff", "--job", "--", "."], 2, "refused path: ."),
    ("refused: an absolute path", ["show", "head:/etc/passwd"], 2, "refused path: head:/etc/passwd"),
    ("refused: an empty path component", ["show", "head:docs//guide.md"], 2, "refused path: head:docs//guide.md"),
    ("refused: a trailing slash", ["show", "head:docs/"], 2, "refused path: head:docs/"),
    ("refused: an empty path", ["diff", "--job", "--", ""], 2, 'refused path: ""'),
    ("usage: no command", [], 2, USAGE),
    ("usage: an unknown command", ["blame"], 2, USAGE),
    ("usage: two revisions for log", ["log", "base", "head"], 2, USAGE),
    ("usage: one revision for diff", ["diff", "base"], 2, USAGE),
    ("usage: --job with a revision", ["diff", "--job", "base"], 2, USAGE),
    (
        "usage: two output forms",
        ["diff", "--numstat", "--raw", "--job"],
        2,
        "at most one of --numstat, --name-only and --raw",
    ),
    ("usage: paths for show", ["show", "base", "--", "README.md"], 2, USAGE),
    ("unknown: a dangling commit", ["show", "{=D}"], 3, "unknown revision or path: {=D}"),
    ("unknown: a dangling commit in a range", ["log", "{=D}..head"], 3, "unknown revision or path: {=D}..head"),
    ("unknown: an id that names nothing", ["diff", "{=zero}", "head"], 3, "unknown revision or path: {=zero}"),
    ("unknown: a blob id", ["show", "{=readme0}"], 3, "unknown revision or path: {=readme0}"),
    ("unknown: a missing path", ["show", "head:missing.txt"], 3, "unknown revision or path: head:missing.txt"),
    ("unknown: a path below a file", ["show", "head:README.md/x"], 3, "unknown revision or path: head:README.md/x"),
    (
        "unknown: a path with quoting in the message",
        ["show", 'head:we"ird'],
        3,
        'unknown revision or path: "head:we\\"ird"',
    ),
    ("set-up: no vcs.json", ["log"], 4, "missing job input .mintswarm/job/inputs/vcs.json", None),
    (
        "set-up: vcs.json names a branch",
        ["log"],
        4,
        "base in .mintswarm/job/inputs/vcs.json is not a full commit id",
        '{"base": "master"}',
    ),
    (
        "set-up: vcs.json is not JSON",
        ["diff", "--job"],
        4,
        "invalid job input .mintswarm/job/inputs/vcs.json",
        "not json",
    ),
    (
        "vcs failure: a repository attributes file that makes a binary file text",
        ["diff", "--numstat", "--job", "--", "bin.dat"],
        5,
        "vcs failure",
        DEFAULT_VCS_JSON,
        "* diff\n",
    ),
    (
        "vcs failure: a repository attributes file that makes every file binary",
        ["diff", "--job", "--", "README.md"],
        5,
        "vcs failure",
        DEFAULT_VCS_JSON,
        "* -diff\n",
    ),
    (
        "set-up: vcs.json has no base",
        ["show", "head"],
        4,
        "invalid job input .mintswarm/job/inputs/vcs.json",
        '{"head": "{=J2}"}',
    ),
    (
        "set-up: vcs.json names a missing commit",
        ["log"],
        4,
        "base in .mintswarm/job/inputs/vcs.json is not a full commit id",
        '{"base": "{=zero}"}',
    ),
    (
        "set-up: base and head share no history",
        ["log"],
        4,
        "base and head do not have exactly one merge base",
        '{"base": "{=O}"}',
    ),
    (
        "set-up: vcs.json names a tree",
        ["log"],
        4,
        "base in .mintswarm/job/inputs/vcs.json is not a full commit id",
        '{"base": "{=Ftree}"}',
    ),
]
CASES += [
    dict(
        name=e[0],
        argv=e[1],
        exit=e[2],
        stderr=f"msw-vcs: {e[3]}\n",
        vcs_json=e[4] if len(e) > 4 else DEFAULT_VCS_JSON,
        **({"attributes": e[5]} if len(e) > 5 else {}),
    )
    for e in ERRORS
]


def assigned_tuple(path: Path, name: str) -> tuple:
    """The literal tuple assigned to name at the top level of a Python file, read without running it."""
    for node in ast.parse(path.read_text(encoding="utf-8")).body:
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == name for t in node.targets):
            return tuple(ast.literal_eval(node.value))
    raise KeyError(f"{name} not found in {path}")


def check_diff_options() -> list:
    floor = tuple(o for o in assigned_tuple(FLOOR, "DIFF_OPTIONS") if o != "-z")
    shim = assigned_tuple(TOOL, "DIFF_OPTIONS")
    return [] if floor == shim else [f"the shim's DIFF_OPTIONS {shim} differ from the landing floor's {floor}"]


def main() -> int:
    failed = 0
    problems = check_diff_options()
    print(f"{'FAIL' if problems else 'ok  '} diff options equal the landing floor's")
    for problem in problems:
        print(f"    {problem}")
    failed += bool(problems)
    with tempfile.TemporaryDirectory(prefix="msw-vcs-selftest.") as tmp:
        fixture = Fixture(Path(tmp))
        for case in CASES:
            fixture.arrange(case)
            try:
                out, err, code = fixture.tool(case["argv"], case.get("vcs_json", DEFAULT_VCS_JSON))
            finally:
                fixture.restore(case)
            want_out = fixture.fill(case.get("stdout", "")).encode("utf-8")
            want_err = fixture.fill(case.get("stderr", "")).encode("utf-8")
            want_code = case.get("exit", 0)
            problems = []
            if code != want_code:
                problems.append(f"exit code {code}, want {want_code}")
            if err != want_err:
                problems.append(f"stderr {err!r}, want {want_err!r}")
            if out != want_out:
                problems.append(f"stdout {out!r}, want {want_out!r}")
            print(f"{'FAIL' if problems else 'ok  '} {case['name']}")
            for problem in problems:
                print(f"    {problem}")
            failed += bool(problems)
    total = len(CASES) + 1
    print(f"{total - failed} of {total} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
