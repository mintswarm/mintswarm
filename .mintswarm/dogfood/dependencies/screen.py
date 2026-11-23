#!/usr/bin/env python3
"""Screen each package a plan declares, for the plan approval.

    screen.py plan --plan <plan.md> --root <dir> --out <file>

Run by the dispatching session, with network access to the registry hosts
and `api.osv.dev` only. It reads the `new_dependencies` of the plan's
`## Footprint` block and screens every declared package for the version the
plan would bring in: what `cargo add` picks (the newest non-yanked,
non-prerelease version on crates.io, or the newest non-prerelease one when
all are yanked) or what `pnpm add` picks (npm's `dist-tags.latest`). A
`Cargo.toml` manifest means crates.io, a `package.json` manifest npm; any
other manifest is `unscreened`.

For each package it records, under `--out` (`screen.json`):

- `size`: `download_bytes` (crates.io `crate_size`), `unpacked_bytes` and
  `file_count` (npm `dist.unpackedSize` and `dist.fileCount`), each
  `"unavailable"` when the registry does not give it; and `dependencies`:
  `direct`, the non-dev, non-optional dependencies of the version, and
  `transitive`, the packages a bounded walk of the registry dependency data
  (depth DEPTH, at most WALK_REQUESTS requests, each dependency at its newest
  version) finds that are not already in the lockfile nearest above the
  manifest (read through yq), with `approximate` set when the walk was cut
  short by a cap, a failed request or a name it could not ask about;
- `author`: the publisher of the version, the owners or maintainers, the
  date of that publisher's first release of the package, and the previous
  release's publisher with `publisher_changed`;
- `establishment`: the creation date and `age_days`, the number of
  releases, downloads (always `"unavailable"` for npm), the repository link
  and the date of the last release;
- `problems`: the advisories for the version from the advisory source (by
  default OSV, through `advisories.OsvSource`), `yanked` (crates.io),
  `deprecated` (npm) and `typosquat_of`.

`disqualifiers` lists only `advisory`, `yanked`, `deprecated` and
`typosquat`; every other signal is evidence only. A package that cannot be
screened in full (a failed registry request, an advisory query that fails,
an unsupported manifest or an invalid name) has `status: "unscreened"` and
a `reason`, never `status: "screened"`; whatever disqualifier was found
before the failure is still listed.

A name is a likely typosquat when, compared after lower-casing and (for
crates, where the registry treats them alike) mapping `_` to `-`, it is
within Damerau-Levenshtein distance 1 of a `popular` name in
`popular-names.json` (2 for names of 8 or more characters), equals one with
the delimiters `-`, `_` and `.` removed, or is scoped with a scope within
distance 1 of a popular scope. When either name is shorter than SHORT
characters, only a swap of two adjacent characters counts as distance 1,
since nearly every short name is one edit from another. Only `popular`
names are imitated; a name in `popular` or `exempt` (the lockfile names
and known neighbours of popular names) is itself exempt, and so is a scope
that either list already uses.

`request` is the only network path: HTTPS to the four registry hosts of
prefetch.py and `api.osv.dev`, no proxy, no redirect, no credential, the
default verified TLS context, GET everywhere and POST only to
`https://api.osv.dev/v1/querybatch`, and a pause of RATE_DELAY before every
request to a crates.io host. prefetch.py and advisories.py are loaded by file
path, so `python3 -I` works.

Standard library only, apart from running yq. Exit 0 whatever happens to
single packages; 2 on bad arguments, an unreadable plan or footprint, an
unreadable typosquat list or an output that cannot be written.
"""

import argparse
import datetime
import importlib.util
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(name: str):
    spec = importlib.util.spec_from_file_location(f"screen_{name}", HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


P = load("prefetch")
A = load("advisories")

TOOL = "dependency-screen"
RECORD_VERSION = 1
OSV_HOST = "api.osv.dev"
HOSTS = P.HOSTS | {OSV_HOST}
POST_URLS = frozenset({A.QUERYBATCH})
USER_AGENT = "mintswarm-dependency-screen"
TIMEOUT = 30
RATE_DELAY = 1.0
MIB = 1024 * 1024
CRATES_META_CAP = 32 * MIB
INDEX_CAP = 32 * MIB
NPM_META_CAP = 64 * MIB
NPM_VERSION_CAP = 4 * MIB
DEPTH = 3
WALK_REQUESTS = 40
OWNERS = 50
NAMES = HERE / "popular-names.json"
SHORT = 5
ECOSYSTEMS = {"Cargo.toml": "crates.io", "package.json": "npm"}
LOCKFILES = {"crates.io": "Cargo.lock", "npm": "pnpm-lock.yaml"}
DISQUALIFIERS = ("advisory", "yanked", "deprecated", "typosquat")
UNAVAILABLE = "unavailable"
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?")


class InputError(Exception):
    """A condition that exits 2."""


class ScreenError(Exception):
    """A request that failed, was refused or returned too much."""


class PackageError(Exception):
    """A package that cannot be screened."""


# --- the network ------------------------------------------------------------


def check_url(url: str) -> str:
    """The host of url when it is https on an allowed host with no user info or port; ScreenError otherwise."""
    try:
        parts = urllib.parse.urlsplit(url)
    except ValueError as exc:
        raise ScreenError(f"unparsable URL {url!r}") from exc
    if parts.scheme != "https":
        raise ScreenError(f"not https: {url!r}")
    if parts.netloc not in HOSTS:
        raise ScreenError(f"host not allowed: {parts.netloc!r}")
    return parts.netloc


def build(method: str, url: str, body: bytes = None) -> urllib.request.Request:
    headers = {"User-Agent": USER_AGENT, "Accept": "application/json"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    return urllib.request.Request(url, data=body, headers=headers, method=method)


def request(method: str, url: str, cap: int, body: bytes = None) -> bytes:
    """The body of the response, at most cap bytes; the only network path. Every error is a ScreenError."""
    host = check_url(url)
    if method == "GET":
        if body is not None:
            raise ScreenError(f"{url}: a GET carries no body")
    elif method == "POST":
        if url not in POST_URLS or not isinstance(body, bytes):
            raise ScreenError(f"{url}: POST is allowed only to {', '.join(sorted(POST_URLS))}, with a body")
    else:
        raise ScreenError(f"{url}: method {method!r} not allowed")
    if host in P.CRATES_HOSTS:
        time.sleep(RATE_DELAY)
    try:
        with P.opener().open(build(method, url, body), timeout=TIMEOUT) as response:
            chunks, size = [], 0
            while size <= cap:
                chunk = response.read(min(cap + 1 - size, MIB))
                if not chunk:
                    break
                chunks.append(chunk)
                size += len(chunk)
    except urllib.error.HTTPError as exc:
        raise ScreenError(f"{url}: HTTP {exc.code}") from exc
    except Exception as exc:
        raise ScreenError(f"{url}: {type(exc).__name__}: {exc}") from exc
    if size > cap:
        raise ScreenError(f"{url}: response over {cap} bytes")
    return b"".join(chunks)


def get_json(url: str, cap: int):
    data = request("GET", url, cap)
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise ScreenError(f"{url}: not JSON") from exc


# --- small helpers ----------------------------------------------------------


def today() -> datetime.date:
    return datetime.datetime.now(datetime.timezone.utc).date()


def day(value):
    """The date of an ISO timestamp, or None."""
    if not isinstance(value, str):
        return None
    try:
        return datetime.date.fromisoformat(value[:10])
    except ValueError:
        return None


def known(value):
    """value, or "unavailable" when it is None."""
    return UNAVAILABLE if value is None else value


def count(value):
    """value when it is a non-negative integer, else "unavailable"."""
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else UNAVAILABLE


def semver(text):
    """(major, minor, patch, prerelease) of a version, or None when it is not semver."""
    match = SEMVER.fullmatch(text) if isinstance(text, str) else None
    if match is None:
        return None
    return int(match.group(1)), int(match.group(2)), int(match.group(3)), bool(match.group(4))


def pick(versions: list, number, yanked) -> dict:
    """The release cargo add would pick from versions: the newest non-yanked non-prerelease one, or the newest
    non-prerelease one when all are yanked; PackageError when there is none."""
    stable = [v for v in versions if semver(number(v)) is not None and not semver(number(v))[3]]
    if not stable:
        raise PackageError("there is no release version")
    candidates = [v for v in stable if not yanked(v)] or stable
    return max(candidates, key=lambda v: semver(number(v))[:3])


def unsafe_manifest(path) -> bool:
    return (
        not isinstance(path, str)
        or not path
        or path.startswith("/")
        or "\\" in path
        or any(part in ("", ".", "..") for part in path.split("/"))
    )


# --- lockfiles --------------------------------------------------------------


def lock_names(root: Path, manifest: str, ecosystem: str) -> tuple:
    """(lockfile path relative to root or None, names in it, readable) for the lockfile nearest above manifest."""
    parts = manifest.split("/")[:-1]
    for depth in range(len(parts), -1, -1):
        relative = "/".join(parts[:depth] + [LOCKFILES[ecosystem]])
        path = root / relative
        if not path.is_file():
            continue
        try:
            data = path.read_bytes()
        except OSError:
            return relative, set(), False
        if ecosystem == "crates.io":
            lock = P.run_yq(data)
            tables = lock.get("package") if isinstance(lock, dict) else None
            if not isinstance(tables, list):
                return relative, set(), False
            return (
                relative,
                {normal(t["name"], ecosystem) for t in tables if isinstance(t, dict) and t.get("name")},
                True,
            )
        lock = yaml(data)
        packages = lock.get("packages") if isinstance(lock, dict) else None
        if packages is None:
            packages = {}
        if not isinstance(packages, dict):
            return relative, set(), False
        names = set()
        for key in packages:
            bare = str(key).split("(", 1)[0].lstrip("/")
            at = bare.rfind("@")
            if at > 0:
                names.add(normal(bare[:at], ecosystem))
        return relative, names, True
    return None, set(), True


def yaml(data: bytes):
    """data, YAML, as JSON through yq on stdin; None when yq cannot read it."""
    try:
        proc = subprocess.run(["yq", "-p=yaml", "-o=json", "."], input=data, capture_output=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    try:
        return json.loads(proc.stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError):
        return None


# --- typosquats -------------------------------------------------------------


def normal(name: str, ecosystem: str) -> str:
    name = name.lower()
    return name.replace("_", "-") if ecosystem == "crates.io" else name


def bare(name: str) -> str:
    return re.sub(r"[-_.]", "", name.lower())


def distance(a: str, b: str, limit: int) -> int:
    """The Damerau-Levenshtein (optimal string alignment) distance of a and b, or limit + 1 when it is over limit."""
    if abs(len(a) - len(b)) > limit:
        return limit + 1
    before, row = None, list(range(len(b) + 1))
    for i in range(1, len(a) + 1):
        current = [i] + [0] * len(b)
        for j in range(1, len(b) + 1):
            cost = 0 if a[i - 1] == b[j - 1] else 1
            current[j] = min(row[j] + 1, current[j - 1] + 1, row[j - 1] + cost)
            if i > 1 and j > 1 and a[i - 1] == b[j - 2] and a[i - 2] == b[j - 1]:
                current[j] = min(current[j], before[j - 2] + 1)
        if min(current) > limit:
            return limit + 1
        before, row = row, current
    return row[-1] if row[-1] <= limit else limit + 1


def scope(name: str):
    if name.startswith("@") and "/" in name:
        return name[1:].split("/", 1)[0]
    return None


def transposed(a: str, b: str) -> bool:
    """Whether b is a with exactly one pair of adjacent characters swapped."""
    if len(a) != len(b):
        return False
    apart = [i for i in range(len(a)) if a[i] != b[i]]
    return len(apart) == 2 and apart[1] == apart[0] + 1 and a[apart[0]] == b[apart[1]] and a[apart[1]] == b[apart[0]]


def close(wanted: str, key: str):
    """The edit distance of wanted from key when the rule counts it as close, or None."""
    if min(len(wanted), len(key)) < SHORT:
        return 1 if transposed(wanted, key) else None
    limit = 2 if len(wanted) >= 8 else 1
    found = distance(wanted, key, limit)
    return found if found <= limit else None


def typosquat_of(name: str, ecosystem: str, popular: list, exempt: list = ()):
    """The popular name (or `@scope/*`) that name likely imitates, or None; names on either list are exempt."""
    table = {}
    for item in popular:
        table.setdefault(normal(item, ecosystem), item)
    known = set(table) | {normal(item, ecosystem) for item in exempt}
    wanted = normal(name, ecosystem)
    if wanted in known:
        return None
    for key in sorted(table):
        if bare(wanted) == bare(key):
            return table[key]
    best = None
    for key in sorted(table):
        found = close(wanted, key)
        if found is not None and (best is None or found < best[0]):
            best = (found, key)
    if best is not None:
        return table[best[1]]
    own = scope(wanted)
    if own is not None and own not in {scope(key) for key in known}:
        for other in sorted({scope(key) for key in table if scope(key) is not None}):
            if distance(own, other, 1) <= 1:
                return f"@{other}/*"
    return None


def popular_names(path: Path = NAMES) -> dict:
    """{ecosystem: {"popular": [...], "exempt": [...]}} from the typosquat list; InputError when it is unusable."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        raise InputError(f"cannot read {path}: {exc}") from exc

    def names(kind: str, ecosystem: str):
        group = data.get(kind) if isinstance(data, dict) else None
        found = group.get(ecosystem) if isinstance(group, dict) else None
        if not isinstance(found, list) or not all(isinstance(n, str) for n in found):
            raise InputError(f"{path} has no {kind} {ecosystem} name list")
        return found

    return {e: {"popular": names("popular", e), "exempt": names("exempt", e)} for e in ("crates.io", "npm")}


# --- crates.io --------------------------------------------------------------


def index_url(name: str) -> str:
    lower = name.lower()
    if len(lower) <= 2:
        prefix = str(len(lower))
    elif len(lower) == 3:
        prefix = f"3/{lower[0]}"
    else:
        prefix = f"{lower[:2]}/{lower[2:4]}"
    return f"https://index.crates.io/{prefix}/{lower}"


def index_releases(name: str) -> list:
    data = request("GET", index_url(name), INDEX_CAP)
    releases = []
    for line in data.decode("utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            release = json.loads(line)
        except (ValueError, RecursionError) as exc:
            raise ScreenError(f"{index_url(name)}: a line is not JSON") from exc
        if isinstance(release, dict):
            releases.append(release)
    return releases


def runtime_crates(release: dict) -> list:
    """The non-dev, non-optional dependency names of an index release."""
    deps = release.get("deps")
    names = []
    for dep in deps if isinstance(deps, list) else []:
        if not isinstance(dep, dict) or dep.get("kind") == "dev" or dep.get("optional"):
            continue
        name = dep.get("package") or dep.get("name")
        if isinstance(name, str) and name not in names:
            names.append(name)
    return names


def crate_dependencies(name: str, version: str = None) -> list:
    """The runtime dependencies of a crate version, or of the release cargo add would pick."""
    releases = index_releases(name)
    if version is None:
        release = pick(releases, lambda r: r.get("vers"), lambda r: r.get("yanked") is True)
    else:
        release = next((r for r in releases if r.get("vers") == version), None)
        if release is None:
            raise ScreenError(f"the index has no {name} {version}")
    return runtime_crates(release)


def screen_crate(name: str) -> dict:
    if not P.CRATE_NAME.fullmatch(name):
        raise PackageError("the name breaks the crates.io name rule")
    api = f"https://crates.io/api/v1/crates/{urllib.parse.quote(name, safe='')}"
    meta = P.mapping(get_json(api, CRATES_META_CAP))
    owners = P.mapping(get_json(f"{api}/owners", CRATES_META_CAP)).get("users")
    info, versions = P.mapping(meta.get("crate")), meta.get("versions")
    if not isinstance(versions, list) or not versions:
        raise PackageError("the crates.io response has no versions")
    versions = [v for v in versions if isinstance(v, dict) and isinstance(v.get("num"), str)]
    chosen = pick(versions, lambda v: v.get("num"), lambda v: v.get("yanked") is True)
    version = chosen["num"]
    if not P.VERSION.fullmatch(version):
        raise PackageError("the version breaks the version rule")

    def by(release):
        return P.scalar(P.mapping(release.get("published_by")).get("login"))

    by_date = sorted(versions, key=lambda v: str(v.get("created_at") or ""))
    publisher = by(chosen)
    earlier = [v for v in by_date if str(v.get("created_at") or "") < str(chosen.get("created_at") or "")]
    previous = earlier[-1] if earlier else None
    own = [v.get("created_at") for v in by_date if publisher is not None and by(v) == publisher]
    return {
        "version": version,
        "size": {
            "download_bytes": count(chosen.get("crate_size")),
            "unpacked_bytes": UNAVAILABLE,
            "file_count": UNAVAILABLE,
        },
        "direct": crate_dependencies(name, version),
        "author": author(
            publisher,
            [P.scalar(u.get("login")) for u in (owners if isinstance(owners, list) else []) if isinstance(u, dict)],
            own[0] if own else None,
            previous is not None,
            by(previous) if previous is not None else None,
        ),
        "establishment": {
            "created": known(P.scalar(info.get("created_at"))),
            "releases": len(versions),
            "downloads": count(info.get("downloads")),
            "repository": P.scalar(info.get("repository")),
            "last_release": known(P.scalar(by_date[-1].get("created_at"))),
        },
        "yanked": chosen.get("yanked") is True,
        "deprecated": None,
    }


# --- npm --------------------------------------------------------------------


def npm_url(name: str) -> str:
    return P.NPM_TARBALLS + name.replace("/", "%2f")


def npm_dependencies(name: str) -> list:
    release = P.mapping(get_json(f"{npm_url(name)}/latest", NPM_VERSION_CAP))
    return sorted(str(k) for k in P.mapping(release.get("dependencies")))


def screen_npm(name: str) -> dict:
    if len(name) > 214 or not P.NPM_NAME.fullmatch(name):
        raise PackageError("the name breaks the npm name rule")
    packument = P.mapping(get_json(npm_url(name), NPM_META_CAP))
    versions, times = P.mapping(packument.get("versions")), P.mapping(packument.get("time"))
    version = P.mapping(packument.get("dist-tags")).get("latest")
    if not isinstance(version, str) or not P.VERSION.fullmatch(version):
        raise PackageError("the packument has no valid dist-tags.latest")
    release = versions.get(version)
    if not isinstance(release, dict):
        raise PackageError(f"version {version} is not in the packument")
    dist = P.mapping(release.get("dist"))

    def by(number):
        return P.scalar(P.mapping(P.mapping(versions.get(number)).get("_npmUser")).get("name"))

    by_date = sorted((v for v in versions if isinstance(times.get(v), str)), key=lambda v: times[v])
    publisher = by(version)
    earlier = [v for v in by_date if times[v] < str(times.get(version) or "")]
    previous = earlier[-1] if earlier else None
    own = [times[v] for v in by_date if publisher is not None and by(v) == publisher]
    repository = release.get("repository") or packument.get("repository")
    deprecated = release.get("deprecated")
    return {
        "version": version,
        "size": {
            "download_bytes": UNAVAILABLE,
            "unpacked_bytes": count(dist.get("unpackedSize")),
            "file_count": count(dist.get("fileCount")),
        },
        "direct": sorted(str(k) for k in P.mapping(release.get("dependencies"))),
        "author": author(
            publisher,
            [P.scalar(m.get("name")) for m in (packument.get("maintainers") or []) if isinstance(m, dict)],
            own[0] if own else None,
            previous is not None,
            by(previous) if previous is not None else None,
        ),
        "establishment": {
            "created": known(P.scalar(times.get("created"))),
            "releases": len(versions),
            "downloads": UNAVAILABLE,
            "repository": P.scalar(P.mapping(repository).get("url") if isinstance(repository, dict) else repository),
            "last_release": known(P.scalar(times[by_date[-1]]) if by_date else None),
        },
        "yanked": None,
        "deprecated": (P.scalar(deprecated) if isinstance(deprecated, str) else "deprecated") if deprecated else None,
    }


# --- one package ------------------------------------------------------------


def author(publisher, owners: list, first, has_previous: bool, previous) -> dict:
    changed = None
    if has_previous and publisher is not None and previous is not None:
        changed = previous != publisher
    return {
        "publisher": known(publisher),
        "owners": sorted(o for o in owners[:OWNERS] if isinstance(o, str)),
        "publisher_first_release": known(P.scalar(first)),
        "previous_publisher": known(previous) if has_previous else None,
        "publisher_changed": changed,
    }


def walk(ecosystem: str, root: str, direct: list, lockfile: set) -> tuple:
    """(new transitive packages, approximate) from a walk of the registry dependency data."""
    dependencies_of = crate_dependencies if ecosystem == "crates.io" else npm_dependencies
    valid = P.CRATE_NAME if ecosystem == "crates.io" else P.NPM_NAME
    seen = {normal(root, ecosystem)}
    found, approximate, requests = set(), False, 0
    frontier = []
    for name in direct:
        key = normal(name, ecosystem)
        if key not in lockfile and key not in seen:
            seen.add(key)
            found.add(key)
            frontier.append(name)
    for _ in range(1, DEPTH):
        following = []
        for name in frontier:
            if requests >= WALK_REQUESTS:
                approximate = True
                break
            if not valid.fullmatch(name):
                approximate = True
                continue
            requests += 1
            try:
                deps = dependencies_of(name)
            except (ScreenError, PackageError):
                approximate = True
                continue
            for dep in deps:
                key = normal(dep, ecosystem)
                if key not in lockfile and key not in seen:
                    seen.add(key)
                    found.add(key)
                    following.append(dep)
        frontier = following
        if not frontier:
            break
    else:
        approximate = approximate or bool(frontier)
    return len(found), approximate


def screen_package(dep: dict, root: Path, names: dict) -> dict:
    """The record of one declared package, before advisories; never raises."""
    name, manifest = dep["name"], dep["manifest"]
    ecosystem = ECOSYSTEMS.get(manifest.rsplit("/", 1)[-1]) if isinstance(manifest, str) else None
    record = {
        "name": name,
        "manifest": manifest,
        "ecosystem": ecosystem,
        "version": None,
        "status": "screened",
        "reason": "",
        "disqualifiers": [],
        "size": None,
        "dependencies": None,
        "author": None,
        "establishment": None,
        "problems": None,
    }
    if ecosystem is None:
        return unscreened(record, "unsupported manifest")
    if unsafe_manifest(manifest):
        return unscreened(record, "the manifest path is not a plain relative path")
    record["problems"] = {
        "advisories": None,
        "yanked": None,
        "deprecated": None,
        "typosquat_of": typosquat_of(name, ecosystem, names[ecosystem]["popular"], names[ecosystem]["exempt"]),
    }
    try:
        found = screen_crate(name) if ecosystem == "crates.io" else screen_npm(name)
        lockfile, known_names, readable = lock_names(root, manifest, ecosystem)
        transitive, approximate = walk(ecosystem, name, found["direct"], known_names)
    except (ScreenError, PackageError) as exc:
        return unscreened(record, str(exc)[: P.TEXT_CAP])
    except Exception as exc:
        return unscreened(record, f"{type(exc).__name__}: {str(exc)[: P.TEXT_CAP]}")
    created = day(found["establishment"]["created"])
    found["establishment"]["age_days"] = (today() - created).days if created else UNAVAILABLE
    record.update(
        version=found["version"],
        size=found["size"],
        dependencies={
            "direct": len(found["direct"]),
            "transitive": transitive,
            "approximate": approximate or not readable,
            "lockfile": lockfile,
        },
        author=found["author"],
        establishment=found["establishment"],
    )
    record["problems"].update(yanked=found["yanked"], deprecated=found["deprecated"])
    return record


def unscreened(record: dict, reason: str) -> dict:
    record.update(status="unscreened", reason=reason or "cannot be screened")
    return record


def disqualify(record: dict) -> None:
    problems = record["problems"] or {}
    hits = {
        "advisory": bool(problems.get("advisories")),
        "yanked": problems.get("yanked") is True,
        "deprecated": bool(problems.get("deprecated")),
        "typosquat": problems.get("typosquat_of") is not None,
    }
    record["disqualifiers"] = [d for d in DISQUALIFIERS if hits[d]]


def add_advisories(records: list, source) -> None:
    """Fill each screened record's advisories from source; a record whose query fails becomes unscreened."""
    keys = {id(r): (r["ecosystem"], r["name"], r["version"]) for r in records if r["status"] == "screened"}
    if not keys:
        return
    try:
        answer = source.query(list(dict.fromkeys(keys.values())))
        if not isinstance(answer, dict):
            raise A.AdvisoryError("the advisory source gave no answer")
    except Exception as exc:
        for record in records:
            if id(record) in keys:
                unscreened(record, f"advisory query failed: {str(exc)[: P.TEXT_CAP]}")
        return
    for record in records:
        if id(record) not in keys:
            continue
        found = answer.get(keys[id(record)])
        if isinstance(found, list) and all(isinstance(a, A.Advisory) for a in found):
            record["problems"]["advisories"] = [a.record() for a in found]
        else:
            why = str(found) if isinstance(found, Exception) else "no answer for this package"
            unscreened(record, f"advisory query failed: {why[: P.TEXT_CAP]}")


def screen(deps: list, root: Path, names: dict, source) -> dict:
    records = [screen_package(dep, root, names) for dep in deps]
    add_advisories(records, source)
    for record in records:
        disqualify(record)
    return {
        "tool": TOOL,
        "version": RECORD_VERSION,
        "mode": "plan",
        "date": today().isoformat(),
        "advisory_source": source.name,
        "packages": records,
    }


# --- the plan ---------------------------------------------------------------


def new_dependencies(path: str) -> list:
    """The footprint's new_dependencies; InputError when the plan or its footprint cannot be read."""
    try:
        text = Path(path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise InputError(f"cannot read the plan {path}: {exc}") from exc
    lines = [line.rstrip("\r") for line in text.split("\n")]
    sections = [i for i, line in enumerate(lines) if line == "## Footprint"]
    if len(sections) != 1:
        raise InputError(f"the plan has {len(sections)} '## Footprint' sections, expected 1")
    start = next((i for i in range(sections[0] + 1, len(lines)) if lines[i] == "```json"), None)
    end = next((i for i in range(start + 1, len(lines)) if lines[i] == "```"), None) if start is not None else None
    if end is None:
        raise InputError("the footprint has no closed ```json block")
    try:
        document = json.loads("\n".join(lines[start + 1 : end]))
    except (ValueError, RecursionError) as exc:
        raise InputError(f"the footprint is not JSON: {exc}") from exc
    deps = P.mapping(P.mapping(document).get("footprint")).get("new_dependencies")
    if not isinstance(deps, list) or not all(
        isinstance(d, dict) and set(d) == {"name", "manifest"} and all(isinstance(v, str) for v in d.values())
        for d in deps
    ):
        raise InputError("footprint.new_dependencies must be a list of {name, manifest} strings")
    return deps


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise InputError(message)


def main(argv: list, source=None) -> int:
    parser = Parser(prog="screen.py", add_help=False)
    commands = parser.add_subparsers(dest="command", parser_class=Parser)
    plan = commands.add_parser("plan", add_help=False)
    for option in ("--plan", "--root", "--out"):
        plan.add_argument(option, required=True)
    try:
        args = parser.parse_args(argv)
        if args.command != "plan":
            raise InputError("the command must be 'plan'")
        root = Path(args.root)
        if not root.is_dir():
            raise InputError(f"--root {root} is not a directory")
        deps = new_dependencies(args.plan)
        names = popular_names()
        record = screen(deps, root, names, source if source is not None else A.OsvSource(request))
        try:
            Path(args.out).write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        except OSError as exc:
            raise InputError(f"cannot write {args.out}: {exc}") from exc
    except InputError as exc:
        print(f"screen.py: {exc}", file=sys.stderr)
        return 2
    packages = record["packages"]
    unscreened_count = sum(p["status"] == "unscreened" for p in packages)
    disqualified = sum(bool(p["disqualifiers"]) for p in packages)
    print(
        f"{len(packages) - unscreened_count} screened, {unscreened_count} unscreened,"
        f" {disqualified} with disqualifiers: {args.out}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
