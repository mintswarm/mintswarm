#!/usr/bin/env python3
"""Keep one gates or review round's results under its round number.

    keep-round.py gates N --head SHA --rounds DIR
    keep-round.py review N --head SHA --rounds DIR [--no-address]

Dogfooding stand-in for the runner's round history. The gates and review
stages overwrite their documents each round, and report-gates.py --clean
deletes the previous round's reports, so each round is copied here before
that happens. Paths other than DIR are relative to the working directory,
which is the worktree root. N is a positive integer without leading zeros;
gate and review rounds are numbered independently, each from 1. The options
come after `kind N`, in any order, each once.

A gates round copies the round's input, .mintswarm/job/inputs/gates.json,
as gates.input.json, so that collect-facts.py can apply every condition of
run-gates.py --verdict, including the gate count and names. It also copies
whichever of .mintswarm/job/outputs/gates.ctrf.json and
.mintswarm/job/outputs/gates.md exist. A round with neither holds only the
input, with a warning, so that a gates stage that failed still counts as a
round. It refuses when inputs/gates.json is missing, or when either output
is older than it (a report or document left from an earlier round), which
is run-gates.py --verdict's staleness condition, checked here because the
kept copies' own times say nothing.

A review round copies .mintswarm/job/outputs/review.md, which is required
and must be titled `# Review round N:`, and .mintswarm/job/outputs/address.md
when it exists and is titled `# Address round N:` (address round N answers
review round N's findings). An address document with any other title is
refused, unless --no-address is given, which ignores the file; the final
review round has no address and is kept with --no-address.

Where rounds are kept. DIR is the job's runner state, outside every
repository; the session passes
${XDG_STATE_HOME:-$HOME/.local/state}/mintswarm/jobs/<worktree name>/rounds/.
DIR must be absolute and not a symbolic link, and neither DIR resolved nor
any of its existing ancestors may be, by identity (st_dev, st_ino), the
working directory, the worktree root (`git rev-parse --show-toplevel`) or
the main checkout (the parent of `git rev-parse --path-format=absolute
--git-common-dir`), so a case variant of a path inside them is refused too.
There is no default inside the worktree. DIR is created with mode 0700 when
missing.

The head. SHA is the full hash of the commit the round judged, and
`git rev-parse --verify --quiet --end-of-options SHA^{commit}` must return
it unchanged. For a gates round it must be HEAD, and the worktree must hold
HEAD's content outside .mintswarm/job/, so that the recorded commit is what
the gates ran on:

- every entry of `git ls-tree -r -z --full-tree HEAD`, split as bytes, is
  in the worktree with the same mode, taken from os.lstat (a symbolic link,
  or a regular file that is executable when its owner may execute it), and
  the same blob id, computed here (`blob <length>\\0` and the content,
  hashed with the algorithm `git rev-parse --show-object-format` names)
  from the file's bytes or the link's target. Paths stay bytes from git's
  output to the file system, so git parses no path; no attribute, filter,
  end-of-line or file-mode setting is consulted, the stat cache is not
  used and no configured filter runs;
- HEAD holds no submodule (gitlink, mode 160000): a submodule's content,
  index and excludes are its own, so none is checked, and a round is not
  kept at a HEAD that has one;
- `git ls-files --others -z --exclude-per-directory=.gitignore`, run
  against a temporary index read from HEAD (`git read-tree HEAD` into a
  file outside the repository, as GIT_INDEX_FILE), lists no untracked path
  outside .mintswarm/job/: only .gitignore files hide paths, never
  .git/info/exclude or core.excludesFile, and a file staged but not
  committed counts as untracked;
- in the repository's own index, `git ls-files -v -z` shows no entry marked
  assume-unchanged (a lower-case tag) or skip-worktree (`S`);
- no untracked .gitignore outside .mintswarm/job/ is itself ignored unless
  a .gitignore that HEAD holds ignores it (`git check-ignore -v`), so an
  untracked .gitignore cannot hide itself and the paths it names.

Every git command runs with the neutral environment floor.py uses (no
system, global or environment config, no replace objects or grafts),
GIT_NO_LAZY_FETCH=1 (git 2.45 and later), and core.fsmonitor and
core.untrackedCache off, core.excludesFile the null device and
core.ignoreCase false, overriding the repository's config. Before any git
command reads an object, a repository whose own config sets
extensions.partialClone, any remote.<name>.promisor to true or any
remote.<name>.partialCloneFilter, in any spelling, is refused: these are
the keys that make a remote a promisor in git 2.44 (its
technical/partial-clone.txt), and reading the repository could fetch a
missing object and run the command that config names. So no fetch and no
configured command runs. Not covered: files that a .gitignore held by HEAD ignores, such as
build output, can still differ.

What this check is. It runs git over a repository and an object store
that the stages can write: the config, the index, the excludes and the
objects. So it closes the bypasses named above and nothing more; it is
not a proof that the worktree is HEAD. A store the stages cannot write is
#87's subject.

For a review round, `git merge-base --is-ancestor SHA HEAD` must succeed:
the round judged the head of the diff.patch it was given, and address
commits may follow it. That SHA is the session's word; it is checked only
for ancestry.

Trust boundary. On one account, keeping DIR outside the worktree and the
main checkout is a convention the stage rules hold, not an OS barrier: any
process of that account can write DIR. Mode 0444 and the sha256 values in
round.json detect only accidental change. The binding to a commit protects
against stale rounds kept honestly: a last gates or review round kept at
an earlier commit is not passed. It does not protect against an edit of
DIR: `head` is a plain field of round.json, which no sha256 covers, so
whoever can edit DIR can set it to the current head. Against that the only
protection is keeping DIR out of the stages' reach.

The round is kept in DIR/<kind>-<N>/ (gates-N or review-N). The files keep
their names and are made mode 0444; round.json records, in this order,
`tool` (`mintswarm-keep-round`), `version` (2), `kind`, `round`, `head`
(SHA), `kept_at` (UTC, YYYY-MM-DDTHH:MM:SSZ) and `files` ({`name`,
`sha256`} per copied file, sorted by name). Everything is written to
DIR/.tmp-<kind>-<N>-<random>/ first, the final name is checked to be
absent, and the temporary directory is renamed onto it, so a round is kept
completely or not at all. The temporary directory is removed on any
failure. A kept round is never changed: keeping the same kind and number
again fails.

Session sequence. report-gates.py's docstring numbers the runner's steps in
every gates round (1 --check, 2 --clean, 3 write inputs/gates.json, 4
dispatch gates and save gates.md, 5 run-gates.py --verdict, 6
report-gates.py). The dogfooding session adds:

- Gates round N is kept with `keep-round.py gates N --head SHA --rounds
  DIR` after step 6, and in any case before step 2 (report-gates.py
  --clean) of gates round N+1. SHA is the HEAD the session dispatched the
  gates at.
- Review round N is kept with `keep-round.py review N --head SHA --rounds
  DIR` after review.md, and address.md if the implementer answered the
  findings, are saved, and before review round N+1 is dispatched, that is,
  before the next review round overwrites review.md. SHA is the head of the
  diff.patch the session gave review round N.
- The last round of each kind is kept before collect-facts.py runs; the
  last review round with --no-address. The last gates round and the last
  review round must be at the final head, or the landing floor hits.
- Before the landing stage is dispatched, the session copies DIR read-only
  to .mintswarm/job/inputs/rounds/ of the landing job, as data for the
  judge only; collect-facts.py and the floor never read that copy.

run-gates.py and report-gates.py are unchanged by this: the verdict of a
round is still run-gates.py --verdict while the round runs, and the kept
copies are read later by collect-facts.py.

Exits 0 when the round is kept; 1 when DIR/<kind>-<N> already exists,
changing nothing; 2 on unusable input (bad arguments, a DIR in the wrong
place, a SHA that is not the head or not its ancestor, a dirty worktree for
a gates round, a missing or stale file, a wrong title), a failing git or an
I/O failure, leaving nothing at the final path. Standard library only,
apart from running git.
"""

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

JOB = Path(".mintswarm/job")
OUTPUTS = JOB / "outputs"
GATES_INPUT = JOB / "inputs" / "gates.json"
TOOL = "mintswarm-keep-round"
VERSION = 2
GATES_FILES = ("gates.ctrf.json", "gates.md")
GATES_INPUT_COPY = "gates.input.json"
REVIEW = "review.md"
ADDRESS = "address.md"
ROUND_NUMBER = re.compile(r"[1-9][0-9]*")
FULL_HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
USAGE = ("usage: keep-round.py gates N --head SHA --rounds DIR | "
         "keep-round.py review N --head SHA --rounds DIR [--no-address]")
JOB_PREFIX = ".mintswarm/job/"
# settings in the repository's config that could make git list fewer untracked files, or run a command
GIT_OVERRIDES = (
    "-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false", "-c", f"core.excludesFile={os.devnull}",
    "-c", "core.ignoreCase=false",
)
FALSE_WORDS = ("false", "no", "off", "0")
GITLINK = "160000"
SYMLINK = "120000"
DROPPED_ENV = (
    "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_EXTERNAL_DIFF", "GIT_DIFF_OPTS", "GIT_REPLACE_REF_BASE",
)
DROPPED_ENV_PREFIXES = ("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")


class InputError(Exception):
    """Unusable input or an I/O failure: exit 2."""


class AlreadyKept(Exception):
    """The round is already kept: exit 1."""


def parse_args(argv: list) -> tuple:
    """Return (kind, number, head, rounds, no_address)."""
    if len(argv) < 2 or argv[0] not in ("gates", "review"):
        raise InputError(USAGE)
    kind, number, rest = argv[0], argv[1], argv[2:]
    if not ROUND_NUMBER.fullmatch(number):
        raise InputError(f"N must be a positive integer, got {number!r}")
    values, i = {}, 0
    while i < len(rest):
        name = rest[i]
        if name in values or name not in ("--head", "--rounds", "--no-address") \
                or (name == "--no-address" and kind != "review"):
            raise InputError(USAGE)
        if name == "--no-address":
            values[name], i = True, i + 1
            continue
        if i + 1 >= len(rest):
            raise InputError(f"{name} needs a value; {USAGE}")
        values[name], i = rest[i + 1], i + 2
    missing = [name for name in ("--head", "--rounds") if name not in values]
    if missing:
        raise InputError(f"missing {', '.join(missing)}; {USAGE}")
    return kind, int(number), values["--head"], values["--rounds"], "--no-address" in values


def neutral_git_config() -> None:
    """Keep system, global and environment config, replace objects and grafts away from git, as floor.py does."""
    for name in list(os.environ):
        if name in DROPPED_ENV or name.startswith(DROPPED_ENV_PREFIXES):
            del os.environ[name]
    os.environ.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull, "GIT_ATTR_NOSYSTEM": "1",
                       "GIT_NO_REPLACE_OBJECTS": "1", "GIT_GRAFT_FILE": os.devnull,
                       "GIT_NO_LAZY_FETCH": "1"})  # git 2.45 and later; older git ignores it


def check_no_promisor() -> None:
    """Refuse a partial clone before any git command reads objects: a missing object would be fetched lazily
    from a promisor remote, running whatever command its config names.

    git 2.44 makes a remote a promisor through extensions.partialClone, a true remote.<name>.promisor or any
    remote.<name>.partialCloneFilter (Documentation/technical/partial-clone.txt, "Using many promisor
    remotes"). `git config --list` gives section and key names in lower case, so every spelling matches.
    """
    code, out = git("config", "-z", "--list")
    if code != 0:
        raise InputError("git config --list failed")
    found = []
    for entry in out.split("\0"):
        key, newline, value = entry.partition("\n")
        if key == "extensions.partialclone":
            found.append(f"extensions.partialClone {value}")
        elif key.startswith("remote.") and key.endswith(".promisor") \
                and (not newline or value.lower() not in FALSE_WORDS):
            found.append(f"{key} {value or 'true'}")
        elif key.startswith("remote.") and key.endswith(".partialclonefilter"):  # whatever its value
            found.append(f"{key} {value}")
    if found:
        raise InputError(f"the repository is a partial clone ({', '.join(found)}): reading its objects could fetch "
                         "them and run a configured command, so no round is kept here")


def git(*args: str, data: bytes = b"", index: str = None) -> tuple:
    """Run git in the working directory with GIT_OVERRIDES, `data` on stdin and, when given, GIT_INDEX_FILE
    `index`: (exit code, stdout as text)."""
    code, out = git_bytes(*args, data=data, index=index)
    return code, out.decode("utf-8", errors="surrogateescape")  # paths round-trip exactly through os.fsencode


def git_bytes(*args: str, data: bytes = b"", index: str = None) -> tuple:
    """As git(), with stdout as bytes."""
    env = dict(os.environ, GIT_INDEX_FILE=index) if index else None
    try:
        proc = subprocess.run(["git", *GIT_OVERRIDES, *args], input=data, capture_output=True, env=env)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    return proc.returncode, proc.stdout


def check_rounds_location(value: str) -> Path:
    """--rounds DIR: absolute, not a symbolic link, and inside neither the worktree nor the main checkout.

    The same text in keep-round.py and collect-facts.py. DIR resolved and each of its existing ancestors
    are compared by identity (st_dev, st_ino) with the working directory, the worktree root and the main
    checkout, so neither a case variant on a case-insensitive file system nor `..` or a symbolic link
    above DIR gets past it.
    """
    path = Path(value)
    if not path.is_absolute():
        raise InputError(f"--rounds must be an absolute path, got {value!r}")
    if path.is_symlink():
        raise InputError(f"--rounds {value} is a symbolic link")
    places = [("the worktree", Path.cwd())]
    for name, args in (("the worktree", ["--show-toplevel"]),
                       ("the main checkout", ["--path-format=absolute", "--git-common-dir"])):
        try:
            proc = subprocess.run(["git", "rev-parse", *args], stdin=subprocess.DEVNULL, capture_output=True)
        except OSError as exc:
            raise InputError(f"cannot run git: {exc}") from exc
        out = os.fsdecode(proc.stdout).strip()  # a path, kept exactly: no replacement characters
        if proc.returncode != 0 or not out:
            raise InputError(f"git rev-parse {args[-1]} failed: the working directory is not a git checkout")
        places.append((name, Path(out).parent if name == "the main checkout" else Path(out)))
    identities = {}
    for name, place in places:
        try:
            info = os.stat(place)
        except OSError as exc:
            raise InputError(f"cannot stat {name} {place}: {exc}") from exc
        identities.setdefault((info.st_dev, info.st_ino), (name, place))
    resolved = path.resolve()
    for candidate in (resolved, *resolved.parents):
        try:
            info = os.stat(candidate)
        except OSError:  # not there yet: an ancestor further up decides
            continue
        found = identities.get((info.st_dev, info.st_ino))
        if found:
            raise InputError(f"--rounds {value} is inside {found[0]} {found[1]}; kept rounds live in runner state")
    return path


def check_head(kind: str, head: str) -> None:
    """Check that SHA is the commit a round of this kind can have judged."""
    if not FULL_HASH.fullmatch(head):
        raise InputError(f"--head must be a full commit hash, got {head!r}")
    code, out = git("rev-parse", "--verify", "--quiet", "--end-of-options", f"{head}^{{commit}}")
    if code != 0 or out.strip() != head:
        raise InputError(f"--head {head} is not a commit of this repository")
    if kind == "review":
        code, _ = git("merge-base", "--is-ancestor", head, "HEAD")
        if code != 0:
            raise InputError(f"--head {head} is not HEAD or an ancestor of it")
        return
    code, out = git("rev-parse", "--verify", "HEAD")
    if code != 0 or out.strip() != head:
        raise InputError(f"--head {head} is not HEAD {out.strip()}: a gates round is kept at the commit it ran on")
    check_worktree()


def git_records(*args: str, index: str = None) -> list:
    """The NUL-separated records of a git command's output; a failing command is unusable input."""
    code, out = git(*args, index=index)
    if code != 0:
        raise InputError(f"git {' '.join(args)} failed")
    return out.split("\0")[:-1] if out else []


def check_worktree() -> None:
    """Refuse a worktree whose content can differ from HEAD outside .mintswarm/job/ (see the docstring)."""
    try:
        with tempfile.TemporaryDirectory(prefix="msw-keep-round-") as scratch:
            index = os.path.join(scratch, "index")
            git_records("read-tree", "HEAD", index=index)
            problems = worktree_problems(index)
    except OSError as exc:
        raise InputError(f"cannot make a temporary index: {exc}") from exc
    if problems:
        raise InputError("the worktree can differ from HEAD outside .mintswarm/job/, so HEAD is not what the "
                         "gates ran on: " + "; ".join(problems))


def worktree_mode(path: bytes):
    """The mode git would record for the worktree's `path`, from os.lstat; None when it holds no file."""
    try:
        info = os.lstat(path)
    except OSError:
        return None
    if stat.S_ISLNK(info.st_mode):
        return SYMLINK
    if stat.S_ISREG(info.st_mode):
        return "100755" if info.st_mode & stat.S_IXUSR else "100644"
    return None


def blob_id(algorithm: str, content: bytes) -> str:
    """The id git gives a blob holding `content`."""
    digest = hashlib.new(algorithm)
    digest.update(b"blob %d\0" % len(content))
    digest.update(content)
    return digest.hexdigest()


def worktree_content(path: bytes, mode: str):
    """The bytes git would store for the worktree's `path`: a symbolic link's target or a file's content."""
    try:
        if mode == SYMLINK:
            return os.readlink(path)
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(fd, "rb") as handle:
            return handle.read()
    except OSError:
        return None


def head_differences() -> tuple:
    """([paths whose worktree content or mode is not HEAD's], [gitlinks in HEAD]), as text, with no conversion.

    HEAD's entries come from `git ls-tree`, split as bytes, and the worktree's blob ids are computed here from
    the bytes on disk, so git parses no path, and no attribute, filter, end-of-line or file-mode setting is
    consulted.
    """
    code, out = git("rev-parse", "--show-object-format")
    algorithm = out.strip()
    if code != 0 or algorithm not in ("sha1", "sha256"):
        raise InputError(f"git rev-parse --show-object-format failed or named {algorithm!r}")
    code, out = git_bytes("ls-tree", "-r", "-z", "--full-tree", "HEAD")
    if code != 0:
        raise InputError("git ls-tree -r HEAD failed")
    changed, gitlinks = [], []
    for record in out.split(b"\0")[:-1] if out else []:
        header, tab, path = record.partition(b"\t")
        parts = header.decode("ascii", errors="replace").split(" ")
        if not tab or len(parts) != 3 or not path:
            raise InputError(f"git ls-tree: unexpected record {record!r}")
        mode, oid = parts[0], parts[2]
        name = path.decode("utf-8", errors="surrogateescape")
        if mode == GITLINK:
            gitlinks.append(name)
        elif worktree_mode(path) != mode:
            changed.append(name)
        else:
            content = worktree_content(path, mode)
            if content is None or blob_id(algorithm, content) != oid:
                changed.append(name)
    return changed, gitlinks


def worktree_problems(index: str) -> list:
    """What makes the worktree differ from HEAD; `index` is a fresh index read from HEAD."""
    problems = []
    changed, gitlinks = head_differences()
    if gitlinks:
        problems.append(f"HEAD holds submodules (gitlinks), whose content is not checked: "
                        f"{', '.join(sorted(gitlinks)[:10])}")
    untracked = git_records("ls-files", "--others", "-z", "--exclude-per-directory=.gitignore", index=index)
    changed += [path for path in untracked if not path.startswith(JOB_PREFIX)]
    if changed:
        problems.append(f"changed or untracked paths: {', '.join(sorted(set(changed))[:10])}")
    # in the repository's own index, assume-unchanged shows as a lower-case tag, skip-worktree as S (s when both)
    flagged = [record[2:] for record in git_records("ls-files", "-v", "-z") if record[0].islower() or record[0] == "S"]
    if flagged:
        problems.append(f"index entries marked assume-unchanged or skip-worktree: {', '.join(sorted(flagged)[:10])}")
    hiding = [path for path in git_records("ls-files", "--others", "--ignored", "-z",
                                           "--exclude-per-directory=.gitignore", index=index)
              if path.rsplit("/", 1)[-1] == ".gitignore" and not path.startswith(JOB_PREFIX)]
    if hiding:  # an untracked .gitignore that is itself ignored: only a tracked one may hide it
        tracked = set(git_records("ls-files", "-z", index=index))
        code, out = git("check-ignore", "-v", "-z", "--no-index", "--stdin",
                        data=b"".join(os.fsencode(path) + b"\0" for path in hiding))
        fields = out.split("\0")[:-1] if out else []
        if code not in (0, 1) or len(fields) % 4:
            raise InputError("git check-ignore failed")
        hiding = [path for source, _, _, path in zip(*[iter(fields)] * 4) if source not in tracked]
        if hiding:
            problems.append(f"untracked .gitignore files that hide paths: {', '.join(sorted(hiding)[:10])}")
    return problems


def mtime_ns(path: Path) -> int:
    try:
        return path.stat().st_mtime_ns
    except OSError as exc:
        raise InputError(f"cannot stat {path}: {exc}") from exc


def read_bytes(path: Path) -> bytes:
    try:
        return path.read_bytes()
    except OSError as exc:
        raise InputError(f"cannot read {path}: {exc}") from exc


def is_file(path: Path) -> bool:
    if path.is_symlink():
        raise InputError(f"{path} is a symbolic link")
    if path.exists() and not path.is_file():
        raise InputError(f"{path} is not a regular file")
    return path.is_file()


def title(data: bytes, path: Path) -> str:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise InputError(f"{path} is not UTF-8: {exc}") from exc
    return text.split("\n", 1)[0]


def gates_files() -> list:
    if not is_file(GATES_INPUT):
        raise InputError(f"{GATES_INPUT} is missing: no gates round is under way")
    started = mtime_ns(GATES_INPUT)
    files = [(GATES_INPUT_COPY, read_bytes(GATES_INPUT))]
    for name in GATES_FILES:
        path = OUTPUTS / name
        if not is_file(path):
            continue
        if mtime_ns(path) < started:
            raise InputError(f"stale {path}: it is older than {GATES_INPUT}")
        files.append((name, read_bytes(path)))
    if len(files) == 1:
        print(f"keep-round: warning: neither {' nor '.join(GATES_FILES)} exists; "
              "keeping a gates round with only its input", file=sys.stderr)
    return files


def review_files(number: int, no_address: bool) -> list:
    review = OUTPUTS / REVIEW
    if not is_file(review):
        raise InputError(f"{review} is missing")
    data = read_bytes(review)
    if not title(data, review).startswith(f"# Review round {number}:"):
        raise InputError(f"{review} is not titled '# Review round {number}:'")
    files = [(REVIEW, data)]
    address = OUTPUTS / ADDRESS
    if not no_address and is_file(address):
        data = read_bytes(address)
        if not title(data, address).startswith(f"# Address round {number}:"):
            raise InputError(
                f"{address} is not titled '# Address round {number}:'; "
                "give --no-address when this review round has no address"
            )
        files.append((ADDRESS, data))
    return files


def write_file(path: Path, data: bytes) -> None:
    with open(path, "xb") as out:
        out.write(data)
        out.flush()
        os.fsync(out.fileno())
    os.chmod(path, 0o444)


def keep(rounds: Path, kind: str, number: int, head: str, files: list) -> Path:
    final = rounds / f"{kind}-{number}"
    try:
        os.makedirs(rounds, mode=0o700, exist_ok=True)
        tmp = Path(tempfile.mkdtemp(prefix=f".tmp-{kind}-{number}-", dir=rounds))
    except OSError as exc:
        raise InputError(f"cannot create a temporary directory in {rounds}: {exc}") from exc
    try:
        record = {
            "tool": TOOL,
            "version": VERSION,
            "kind": kind,
            "round": number,
            "head": head,
            "kept_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "files": [
                {"name": name, "sha256": hashlib.sha256(data).hexdigest()}
                for name, data in sorted(files)
            ],
        }
        for name, data in files:
            write_file(tmp / name, data)
        write_file(tmp / "round.json", (json.dumps(record, indent=2) + "\n").encode("utf-8"))
        os.chmod(tmp, 0o755)
        if os.path.lexists(final):
            raise AlreadyKept(f"{final} already exists")
        try:
            os.rename(tmp, final)
        except OSError as exc:
            if os.path.lexists(final):
                raise AlreadyKept(f"{final} already exists") from exc
            raise
    except BaseException as exc:
        shutil.rmtree(tmp, ignore_errors=True)
        if isinstance(exc, OSError):
            raise InputError(f"cannot keep {final}: {exc}") from exc
        raise
    return final


def keep_round(argv: list) -> int:
    kind, number, head, value, no_address = parse_args(argv)
    neutral_git_config()
    check_no_promisor()
    rounds = check_rounds_location(value)
    check_head(kind, head)
    final = rounds / f"{kind}-{number}"
    if os.path.lexists(final):
        raise AlreadyKept(f"{final} already exists")
    files = gates_files() if kind == "gates" else review_files(number, no_address)
    final = keep(rounds, kind, number, head, files)
    names = ", ".join(name for name, _ in sorted(files)) or "no files"
    print(f"kept {final} at {head}: {names}")
    return 0


def main(argv: list) -> int:
    try:
        return keep_round(argv)
    except AlreadyKept as exc:
        print(f"keep-round: {exc}; a kept round is never changed", file=sys.stderr)
        return 1
    except InputError as exc:
        print(f"keep-round: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as kept or already kept
        print(f"keep-round: unexpected error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
