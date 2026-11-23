#!/usr/bin/env python3
"""List the packages a change adds or upgrades, for the review stage's dependency verdict.

    new-packages.py --base <commit> --head <rev> --out <dir>

Run at the worktree root. Reads `pipeline.dependencies.review` from
`git show <base>:.mintswarm/config.yaml` through `yq -p=yaml -o=json`; the
worktree's config is never opened. With the key absent or `off` it prints
`dependency review: off` and writes nothing; any value other than `agent` or
`off` exits 2.

With `agent`, it lists the dependency-relevant paths `git diff --name-only
--no-renames <base> <head>` reports: files named Cargo.toml, Cargo.lock,
package.json, pnpm-lock.yaml, pnpm-workspace.yaml or .npmrc, and paths
ending in .cargo/config.toml or .cargo/config. With none, it prints
`no dependency change` and writes nothing.

Otherwise it reads each changed Cargo.lock (through `yq -p=toml -o=json`,
checking the shape of every `package` table) and pnpm-lock.yaml (through
`yq -p=yaml -o=json`) at the base and the head; a file absent at a side counts as empty. A package is
reported when its (name, version, source) is in the head lockfile and not
in the base; `previous` holds the base versions of that name, `checksum` the
Cargo.lock `checksum` and `integrity` the pnpm `resolution.integrity` (each
null when the lockfile has none), which the prefetch checks archives
against. Workspace
members and path packages (a Cargo.lock entry without `source`) are part of
the diff itself and are not reported. Problems are a lockfile that cannot be
read (a Cargo.lock top-level key other than `version`, `package`, `metadata`
or `patch` included) and a changed Cargo.toml or package.json with no changed lockfile of
its ecosystem.

It first removes any `<dir>/packages.json` left by an earlier run, so the
off, no-change and error paths leave none. It writes `<dir>/packages.json` with the keys `base`, `head`, `changed`,
`packages` and `problems`, sorted, so the output is deterministic.

Standard library only, apart from running git and yq. Never contacts the
network and executes no repository file. Exit 0 on success, 2 on bad
arguments, git or yq missing, a ref that does not resolve, or a base config
that cannot be read or holds an unknown value.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

CONFIG = ".mintswarm/config.yaml"
MANIFESTS = {"Cargo.toml": "cargo", "package.json": "npm"}
LOCKFILES = {"Cargo.lock": "cargo", "pnpm-lock.yaml": "npm"}
SETTINGS = {"pnpm-workspace.yaml", ".npmrc"}
CARGO_CONFIGS = (".cargo/config.toml", ".cargo/config")
CRATES_IO = {"registry+https://github.com/rust-lang/crates.io-index", "sparse+https://index.crates.io/"}
NPM_REGISTRY = "registry"


class InputError(Exception):
    """A condition that exits 2."""


class LockError(Exception):
    """A lockfile that cannot be read; recorded as a problem."""


CARGO_LOCK_KEYS = {"version", "package", "metadata", "patch"}


# --- git and yq -------------------------------------------------------------


def run(argv: list, data: bytes = None) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, input=data, capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run {argv[0]}: {exc}") from exc


def last_line(proc: subprocess.CompletedProcess) -> str:
    lines = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
    return lines[-1] if lines else f"exit {proc.returncode}"


def resolve(ref: str) -> str:
    proc = run(["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"])
    commit = proc.stdout.decode("ascii", errors="replace").strip()
    if proc.returncode != 0 or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
        raise InputError(f"ref {ref!r} does not resolve to a commit")
    return commit


def to_json(data: bytes, fmt: str = "yaml"):
    proc = run(["yq", f"-p={fmt}", "-o=json", "."], data)
    if proc.returncode != 0:
        raise LockError(f"yq: {last_line(proc)}")
    try:
        return json.loads(proc.stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise LockError(f"yq output is not JSON: {exc}") from exc


def review_mode(base: str) -> str:
    proc = run(["git", "show", f"{base}:{CONFIG}"])
    if proc.returncode != 0:
        raise InputError(f"no {CONFIG} at {base}")
    try:
        config = to_json(proc.stdout)
    except LockError as exc:
        raise InputError(f"{CONFIG} at {base} cannot be read: {exc}") from exc
    if not isinstance(config, dict):
        raise InputError(f"{CONFIG} at {base} is not a mapping")
    pipeline = config.get("pipeline")
    if pipeline is None:
        return "off"
    if not isinstance(pipeline, dict):
        raise InputError("config: pipeline must be a mapping")
    dependencies = pipeline.get("dependencies")
    if dependencies is None:
        return "off"
    if not isinstance(dependencies, dict):
        raise InputError("config: pipeline.dependencies must be a mapping")
    value = dependencies.get("review")
    if value is None or value == "off":
        return "off"
    if value == "agent":
        return "agent"
    raise InputError(f"config: pipeline.dependencies.review must be agent or off, not {json.dumps(value)}")


def changed_paths(base: str, head: str) -> list:
    proc = run(["git", "diff", "--name-only", "--no-renames", "-z", base, head])
    if proc.returncode != 0:
        raise InputError(f"git diff failed: {last_line(proc)}")
    paths = [p.decode("utf-8", errors="surrogateescape") for p in proc.stdout.split(b"\0") if p]
    return sorted(p for p in paths if relevant(p))


def relevant(path: str) -> bool:
    name = path.rsplit("/", 1)[-1]
    if name in MANIFESTS or name in LOCKFILES or name in SETTINGS:
        return True
    return any(path == c or path.endswith("/" + c) for c in CARGO_CONFIGS)


def read_blob(rev: str, path: str) -> bytes:
    """The file at rev, b"" when absent; LockError when it is not a regular file or cannot be read."""
    proc = run(["git", "ls-tree", "-z", rev, "--", path])
    if proc.returncode != 0:
        raise LockError(f"git ls-tree failed at {rev}: {last_line(proc)}")
    if not proc.stdout:
        return b""
    mode = proc.stdout.split(b" ", 1)[0]
    if mode not in (b"100644", b"100755"):
        raise LockError(f"not a regular file at {rev} (mode {mode.decode('ascii', errors='replace')})")
    proc = run(["git", "show", f"{rev}:{path}"])
    if proc.returncode != 0:
        raise LockError(f"git show failed at {rev}: {last_line(proc)}")
    return proc.stdout


# --- Cargo.lock -------------------------------------------------------------


def parse_cargo_lock(data: bytes) -> list:
    """The `package` tables, read by `yq -p=toml`, as dicts with name, version, source and checksum (each
    possibly None) and dependencies.

    Any top-level key other than CARGO_LOCK_KEYS, and any shape other than a list of tables with string name
    and version, an optional string source and checksum and an optional list of string dependencies, is a
    LockError, so no table is dropped silently.
    """
    try:
        data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise LockError("not UTF-8") from exc
    if not data.strip():
        return []
    lock = to_json(data, "toml")
    if not isinstance(lock, dict):
        raise LockError("not a table")
    unknown = sorted(set(lock) - CARGO_LOCK_KEYS)
    if unknown:
        raise LockError(f"unknown top-level key {unknown[0]!r}")
    tables = lock.get("package", [])
    if not isinstance(tables, list):
        raise LockError("package is not an array of tables")
    packages = []
    for number, table in enumerate(tables, 1):
        where = f"package {number}"
        if not isinstance(table, dict):
            raise LockError(f"{where} is not a table")
        name, version = table.get("name"), table.get("version")
        source, dependencies = table.get("source"), table.get("dependencies", [])
        checksum = table.get("checksum")
        if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
            raise LockError(f"{where} has no string name and version")
        if source is not None and not isinstance(source, str):
            raise LockError(f"{where} ({name}) has a source that is not a string")
        if checksum is not None and not isinstance(checksum, str):
            raise LockError(f"{where} ({name}) has a checksum that is not a string")
        if not isinstance(dependencies, list) or not all(isinstance(d, str) for d in dependencies):
            raise LockError(f"{where} ({name}) has dependencies that are not a list of strings")
        packages.append(
            {"name": name, "version": version, "source": source, "checksum": checksum, "dependencies": dependencies}
        )
    return packages


def cargo_entries(data: bytes) -> list:
    """(name, version, source, registry, direct, checksum, integrity) for every package with a source."""
    packages = parse_cargo_lock(data)
    wanted = []
    for member in packages:
        if member["source"] is None:
            for dependency in member["dependencies"]:
                parts = dependency.split(" ")
                wanted.append((parts[0], parts[1] if len(parts) > 1 else None))
    entries = []
    for package in packages:
        if package["source"] is None:
            continue
        name, version = package["name"], package["version"]
        direct = any(n == name and (v is None or v == version) for n, v in wanted)
        source = package["source"]
        entries.append((name, version, source, source in CRATES_IO, direct, package["checksum"], None))
    return entries


# --- pnpm-lock.yaml ---------------------------------------------------------


def split_key(key: str) -> tuple:
    """name and version of a pnpm `packages` key: `name@version`, peer suffix `(…)` cut, a leading `/` dropped."""
    bare = key.split("(", 1)[0]
    if bare.startswith("/"):
        bare = bare[1:]
    at = bare.rfind("@")
    if at <= 0 or at == len(bare) - 1:
        raise LockError(f"unrecognised package key {key!r}")
    return bare[:at], bare[at + 1 :]


def npm_source(resolution) -> tuple:
    """(source, registry) for a pnpm `resolution`."""
    if isinstance(resolution, dict):
        if set(resolution) == {"integrity"}:
            return NPM_REGISTRY, True
        if isinstance(resolution.get("tarball"), str):
            return resolution["tarball"], False
        if resolution.get("type") == "git":
            return f"git+{resolution.get('repo')}#{resolution.get('commit')}", False
        if resolution.get("type") == "directory":
            return f"directory:{resolution.get('directory')}", False
    return json.dumps(resolution, sort_keys=True, separators=(",", ":")), False


def importer_pairs(lock: dict) -> set:
    """(name, version) of every direct dependency of every importer."""
    importers = lock.get("importers")
    if importers is None:
        importers = {".": lock}
    if not isinstance(importers, dict):
        raise LockError("importers is not a mapping")
    pairs = set()
    for importer in importers.values():
        if not isinstance(importer, dict):
            raise LockError("an importer is not a mapping")
        for group in ("dependencies", "devDependencies", "optionalDependencies"):
            deps = importer.get(group) or {}
            if not isinstance(deps, dict):
                raise LockError(f"{group} of an importer is not a mapping")
            for name, spec in deps.items():
                version = spec.get("version") if isinstance(spec, dict) else spec
                if not isinstance(version, str):
                    continue
                version = version.split("(", 1)[0].lstrip("/")
                at = version.rfind("@")
                if at > 0:  # an alias: `real@1.0.0`
                    name, version = version[:at], version[at + 1 :]
                pairs.add((name, version))
    return pairs


def npm_entries(data: bytes) -> list:
    """(name, version, source, registry, direct, checksum, integrity) for every package; checksum is None."""
    if not data.strip():
        return []
    lock = to_json(data)
    if not isinstance(lock, dict):
        raise LockError("not a mapping")
    packages = lock.get("packages") or {}
    if not isinstance(packages, dict):
        raise LockError("packages is not a mapping")
    direct = importer_pairs(lock)
    entries = set()
    for key, entry in packages.items():
        name, version = split_key(key)
        resolution = entry.get("resolution") if isinstance(entry, dict) else None
        source, registry = npm_source(resolution)
        integrity = resolution.get("integrity") if isinstance(resolution, dict) else None
        integrity = integrity if isinstance(integrity, str) else None
        entries.add((name, version, source, registry, (name, version) in direct, None, integrity))
    return sorted(entries, key=lambda e: tuple("" if x is None else x for x in e))


# --- the report -------------------------------------------------------------


def lock_entries(ecosystem: str, data: bytes) -> list:
    return cargo_entries(data) if ecosystem == "cargo" else npm_entries(data)


def report(base: str, head: str, changed: list) -> dict:
    packages, problems = [], []
    for path in changed:
        name = path.rsplit("/", 1)[-1]
        ecosystem = LOCKFILES.get(name)
        if ecosystem is None:
            continue
        sides = {}
        for side, rev in (("base", base), ("head", head)):
            try:
                sides[side] = lock_entries(ecosystem, read_blob(rev, path))
            except LockError as exc:
                problems.append({"path": path, "problem": f"cannot read the {side} lockfile: {exc}"})
        if len(sides) != 2:
            continue
        before = {entry[:3] for entry in sides["base"]}
        for n, v, s, registry, direct, checksum, integrity in sides["head"]:
            if (n, v, s) in before:
                continue
            previous = sorted({entry[1] for entry in sides["base"] if entry[0] == n})
            packages.append(
                {
                    "ecosystem": ecosystem,
                    "lockfile": path,
                    "name": n,
                    "version": v,
                    "previous": previous,
                    "source": s,
                    "registry": registry,
                    "direct": direct,
                    "checksum": checksum,
                    "integrity": integrity,
                }
            )
    locked = {LOCKFILES[p.rsplit("/", 1)[-1]] for p in changed if p.rsplit("/", 1)[-1] in LOCKFILES}
    for path in changed:
        ecosystem = MANIFESTS.get(path.rsplit("/", 1)[-1])
        if ecosystem is not None and ecosystem not in locked:
            lockfile = "Cargo.lock" if ecosystem == "cargo" else "pnpm-lock.yaml"
            problems.append({"path": path, "problem": f"manifest changed with no {lockfile} change"})
    packages.sort(key=lambda p: (p["lockfile"], p["name"], p["version"], p["source"]))
    problems.sort(key=lambda p: (p["path"], p["problem"]))
    return {"base": base, "head": head, "changed": changed, "packages": packages, "problems": problems}


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise InputError(message)


def main(argv: list) -> int:
    parser = Parser(prog="new-packages.py", add_help=False)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    parser.add_argument("--out", required=True)
    try:
        args = parser.parse_args(argv)
        target = Path(args.out) / "packages.json"
        try:
            if target.is_symlink() or target.exists():
                target.unlink()
        except OSError as exc:
            raise InputError(f"cannot remove the stale {target}: {exc}") from exc
        base, head = resolve(args.base), resolve(args.head)
        if review_mode(base) == "off":
            print("dependency review: off")
            return 0
        changed = changed_paths(base, head)
        if not changed:
            print("no dependency change")
            return 0
        result = report(base, head, changed)
    except InputError as exc:
        print(f"new-packages.py: {exc}", file=sys.stderr)
        return 2
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(
        f"{len(result['packages'])} new or upgraded packages, {len(result['problems'])} problems: {out}/packages.json"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
