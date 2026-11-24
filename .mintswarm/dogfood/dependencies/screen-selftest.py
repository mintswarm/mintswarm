#!/usr/bin/env python3
"""Offline self-test of screen.py, the plan-time dependency screen, and advisories.py.

    screen-selftest.py

Needs `yq` on PATH; nothing else, and no network. It loads screen.py and
advisories.py with importlib, makes every socket connection and name lookup
raise, replaces screen.py's `request` with recorded crates.io, npm and OSV
responses held inline, pins the screen date, and calls the module in-process
on `tempfile.mkdtemp` directories.

Cases: each signal (version resolution, sizes, direct and transitive
dependencies with the lockfile skip and the approximate caps, publisher,
owners, first release, previous publisher, creation, age, releases,
downloads, repository, last release, the `"unavailable"` values); each
disqualifier (an advisory, a withdrawn advisory ignored, OSV paging, yanked,
deprecated, a typosquat by distance, delimiter and scope, an exempt listed
name); evidence alone gives no disqualifier; a failed fetch, an OSV error,
a failed advisory detail, an invalid name and an unsupported manifest give
`unscreened`; the transport rules (hosts, HTTPS only, POST only to
querybatch, no proxy, no redirect, no Authorization header, the response
cap, the crates.io pause); a fake `AdvisorySource` in place of `OsvSource`;
the CLI (exit codes, the record shape, the same output on two runs); the
typosquat list, with the known neighbours of popular names (poem, tide, curl,
x25519-dalek, k256, gloo, gix, cbor, preact, nuxt, msw, h3) not flagged;
the review mode on recorded `packages.json`, `prefetch.json` and
`metadata.json` files (one record per lockfile package, transitive ones
included; each disqualifier on a transitive package; each threshold just
under and at its limit: a new package, a new version, a publisher change,
low crates.io downloads, no repository; an `"unavailable"` value and npm
downloads never flagging; a clean package unflagged; a refused or failed
prefetch, a missing `metadata.json` and an OSV error giving `unscreened`
and flagged; no `packages.json`, a stale `screen.json` removed, a malformed
`packages.json` or `prefetch.json`; every request to `https://api.osv.dev`
and none to a registry host);
Python 3.9 syntax, standard-library imports, and AST rules that `request`
is the only function in screen.py that opens a URL, that every
`subprocess.run` in it has a literal list argv starting with "yq", that
`getattr`, `__import__` and `importlib` appear only in `load`, and that
advisories.py opens none.

Prints one `ok` or `FAIL` line per check, then a count, and exits 0 only
when all passed.
"""

import ast
import contextlib
import datetime
import importlib.util
import io
import json
import os
import shutil
import socket
import sys
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "screen.py"
ADVISORIES = HERE / "advisories.py"
STDLIB = frozenset(
    "argparse ast contextlib datetime importlib io json os re shutil socket subprocess sys tempfile time typing"
    " urllib pathlib".split()
)
NETWORK = frozenset(
    {"urlopen", "opener", "build_opener", "open", "create_connection", "HTTPSConnection", "HTTPConnection"}
    | {"fetch", "fetch_json", "socket", "ssl", "http"}
)
TODAY = datetime.date(2024, 1, 1)
NAMES = {
    "crates.io": {"popular": ["serde_json", "tokio"], "exempt": []},
    "npm": {"popular": ["@types/node", "lodash"], "exempt": []},
}
# Legitimate packages one or two edits from a popular name, each with the name it is near.
NEIGHBOURS = {
    "crates.io": {
        "poem": "pem",
        "tide": "time",
        "curl": "url",
        "x25519-dalek": "ed25519-dalek",
        "k256": "p256",
        "gloo": "glob",
        "gix": "nix",
        "cbor": "ctor",
    },
    "npm": {"msw": "ms", "nuxt": "next", "preact": "react", "h3": "he"},
}


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
BLOCKED = []


def blocked(*args, **kwargs):
    BLOCKED.append(args[1:] if args else kwargs)
    raise OSError("network blocked by screen-selftest")


socket.socket.connect = blocked
socket.socket.connect_ex = blocked
socket.getaddrinfo = blocked
socket.create_connection = blocked


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


S = load("screen", SCRIPT)
A = S.A
REAL_REQUEST = S.request
S.RATE_DELAY = 0
S.today = lambda: TODAY


# --- recorded responses -----------------------------------------------------


def dep(name: str, kind: str = "normal", optional: bool = False, package: str = None) -> dict:
    entry = {"name": name, "req": "^1", "kind": kind, "optional": optional, "features": [], "target": None}
    if package is not None:
        entry["package"] = package
    return entry


def release(num: str, created: str, by: str = "alice", yanked: bool = False, size=1000, deps: list = None) -> dict:
    return {"num": num, "created_at": created, "by": by, "yanked": yanked, "size": size, "deps": deps or []}


class Registry:
    """Recorded crates.io, index, npm and OSV responses, served in place of `request`."""

    def __init__(self):
        self.routes = {}
        self.osv = {}
        self.vulns = {}
        self.page = 0
        self.osv_error = None
        self.calls = []
        self.queries = []

    def __call__(self, method: str, url: str, cap: int, body: bytes = None) -> bytes:
        S.check_url(url)
        self.calls.append((method, url))
        if method == "POST":
            if url != A.QUERYBATCH:
                raise S.ScreenError("POST elsewhere")
            return self.batch(body)
        prefix = A.OSV + "/v1/vulns/"
        if url.startswith(prefix):
            entry = self.vulns.get(urllib.parse.unquote(url[len(prefix) :]))
            if entry is None or isinstance(entry, Exception):
                raise entry or S.ScreenError(f"{url}: HTTP 404")
            return json.dumps(entry).encode()
        value = self.routes.get(url)
        if value is None:
            raise S.ScreenError(f"{url}: HTTP 404")
        if isinstance(value, Exception):
            raise value
        return value

    def batch(self, body: bytes) -> bytes:
        if self.osv_error is not None:
            raise self.osv_error
        queries = json.loads(body.decode("utf-8"))["queries"]
        self.queries.append(queries)
        results = []
        for query in queries:
            key = (query["package"]["ecosystem"], query["package"]["name"], query["version"])
            ids = self.osv.get(key, [])
            start = int(query.get("page_token", "0"))
            result = {}
            if self.page:
                chosen = ids[start : start + self.page]
                if start + self.page < len(ids):
                    result["next_page_token"] = str(start + self.page)
            else:
                chosen = ids
            if chosen:
                result["vulns"] = [{"id": i, "modified": "2024-01-01T00:00:00Z"} for i in chosen]
            results.append(result)
        return json.dumps({"results": results}).encode()

    def crate(
        self,
        name: str,
        releases: list,
        created: str = "2020-01-10T00:00:00.000000+00:00",
        downloads=5000,
        repository="https://github.com/example/crate",
        owners=("alice",),
    ) -> None:
        api = f"https://crates.io/api/v1/crates/{name}"
        versions = []
        for item in releases:
            version = {"num": item["num"], "created_at": item["created_at"], "yanked": item["yanked"]}
            if item["size"] is not None:
                version["crate_size"] = item["size"]
            if item["by"] is not None:
                version["published_by"] = {"login": item["by"], "name": item["by"].title()}
            versions.append(version)
        info = {"name": name, "created_at": created, "downloads": downloads, "repository": repository}
        self.routes[api] = json.dumps({"crate": info, "versions": versions}).encode()
        users = [{"login": owner, "kind": "user"} for owner in owners]
        self.routes[f"{api}/owners"] = json.dumps({"users": users}).encode()
        lines = [
            json.dumps({"name": name, "vers": r["num"], "deps": r["deps"], "yanked": r["yanked"], "cksum": "0" * 64})
            for r in releases
        ]
        self.routes[S.index_url(name)] = ("\n".join(lines) + "\n").encode()

    def simple_crate(self, name: str, deps: list = None) -> None:
        self.crate(name, [release("1.0.0", "2020-01-10T00:00:00.000000+00:00", deps=deps)])

    def npm(
        self,
        name: str,
        latest: str,
        versions: dict,
        times: dict,
        maintainers=("carol",),
        created="2019-04-30T00:00:00.000Z",
    ) -> None:
        packument = {
            "name": name,
            "dist-tags": {"latest": latest},
            "versions": versions,
            "time": dict(times, created=created, modified="2024-01-01T00:00:00.000Z"),
            "maintainers": [{"name": m, "email": f"{m}@example.org"} for m in maintainers],
        }
        url = S.npm_url(name)
        self.routes[url] = json.dumps(packument).encode()
        self.routes[f"{url}/latest"] = json.dumps(versions[latest]).encode()


def npm_version(name: str, version: str, by: str = "carol", deps: dict = None, dist: dict = None, **extra) -> dict:
    return dict(
        {"name": name, "version": version, "_npmUser": {"name": by}, "dependencies": deps or {}, "dist": dist or {}},
        **extra,
    )


class Fixture:
    """A repository root with a Cargo.lock and web/pnpm-lock.yaml, and the registry it screens against."""

    def __init__(self, tmp: Path):
        self.root = tmp / "repo"
        (self.root / "web").mkdir(parents=True)
        (self.root / "Cargo.lock").write_text(
            'version = 3\n\n[[package]]\nname = "known_dep"\nversion = "1.0.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n',
            encoding="utf-8",
        )
        (self.root / "web" / "pnpm-lock.yaml").write_text(
            "lockfileVersion: '9.0'\npackages:\n  known-npm@1.0.0:\n    resolution: {integrity: sha512-AA==}\n",
            encoding="utf-8",
        )
        self.reg = Registry()
        reg = self.reg
        # goodcrate: 1.1.0 is chosen over a yanked 1.2.0 and a prerelease; its publisher differs from 1.0.0's.
        reg.crate(
            "goodcrate",
            [
                release("1.0.0", "2020-01-10T00:00:00.000000+00:00", by="alice"),
                release(
                    "1.1.0",
                    "2021-03-01T00:00:00.000000+00:00",
                    by="bob",
                    size=12345,
                    deps=[
                        dep("dep-a"),
                        dep("dep-b", kind="build"),
                        dep("dev-x", kind="dev"),
                        dep("opt-y", optional=True),
                        dep("known-dep"),
                        dep("alias", package="dep-c"),
                    ],
                ),
                release("1.2.0", "2022-01-01T00:00:00.000000+00:00", by="bob", yanked=True),
                release("2.0.0-alpha.1", "2023-01-01T00:00:00.000000+00:00", by="bob"),
            ],
            owners=("bob", "alice"),
        )
        reg.simple_crate("dep-a", [dep("dep-d"), dep("known_dep")])
        reg.simple_crate("dep-b", [dep("dep-a")])
        reg.simple_crate("dep-c")
        reg.simple_crate("dep-d")
        # steady: same publisher throughout, no crate_size, no repository, no publisher on its first release.
        reg.crate(
            "steady",
            [
                release("0.1.0", "2019-01-01T00:00:00.000000+00:00", by=None),
                release("0.2.0", "2019-06-01T00:00:00.000000+00:00", by="dan"),
                release("0.3.0", "2020-06-01T00:00:00.000000+00:00", by="dan", size=None),
            ],
            repository=None,
            downloads=None,
            owners=("dan",),
        )
        reg.crate("deep", [release("1.0.0", "2020-01-10T00:00:00.000000+00:00", deps=[dep("chain-1")])])
        for number in range(1, 5):
            reg.simple_crate(f"chain-{number}", [dep(f"chain-{number + 1}")])
        reg.crate("bigcrate", [release("1.0.0", "2023-12-20T00:00:00.000000+00:00", by="eve", size=900_000_000)])
        reg.crate(
            "yankedcrate",
            [
                release("1.0.0", "2020-01-10T00:00:00.000000+00:00", yanked=True),
                release("1.1.0", "2020-02-10T00:00:00.000000+00:00", yanked=True),
            ],
        )
        for name in ("vulncrate", "withdrawncrate", "pagedcrate", "brokenvuln", "tokoi", "serdejson", "serde-json"):
            reg.simple_crate(name)
        reg.routes["https://crates.io/api/v1/crates/flaky"] = S.ScreenError("HTTP 500")
        reg.osv[("crates.io", "vulncrate", "1.0.0")] = ["RUSTSEC-2099-0001"]
        reg.osv[("crates.io", "withdrawncrate", "1.0.0")] = ["GHSA-wwww-wwww-wwww"]
        reg.osv[("crates.io", "pagedcrate", "1.0.0")] = ["RUSTSEC-2099-0003", "RUSTSEC-2099-0002", "GHSA-pppp"]
        reg.osv[("crates.io", "brokenvuln", "1.0.0")] = ["RUSTSEC-2099-0009"]
        reg.vulns["RUSTSEC-2099-0001"] = {
            "id": "RUSTSEC-2099-0001",
            "summary": "Use after free in vulncrate",
            "aliases": ["CVE-2099-1", "GHSA-aaaa-bbbb-cccc"],
        }
        reg.vulns["GHSA-wwww-wwww-wwww"] = {"id": "GHSA-wwww-wwww-wwww", "summary": "x", "withdrawn": "2024-01-01"}
        for ident in ("RUSTSEC-2099-0003", "RUSTSEC-2099-0002", "GHSA-pppp"):
            reg.vulns[ident] = {"id": ident, "details": f"details of {ident}"}
        reg.vulns["RUSTSEC-2099-0009"] = S.ScreenError("HTTP 503")
        # npm
        reg.npm(
            "good-npm",
            "2.0.0",
            {
                "1.0.0": npm_version("good-npm", "1.0.0"),
                "2.0.0": npm_version(
                    "good-npm",
                    "2.0.0",
                    deps={"dep-n1": "^1.0.0", "known-npm": "^1.0.0"},
                    dist={"unpackedSize": 2048, "fileCount": 7},
                    repository={"type": "git", "url": "git+https://github.com/example/good-npm.git"},
                ),
                "3.0.0-rc.1": npm_version("good-npm", "3.0.0-rc.1"),
            },
            {
                "1.0.0": "2019-05-01T00:00:00.000Z",
                "2.0.0": "2020-06-01T00:00:00.000Z",
                "3.0.0-rc.1": "2021-07-01T00:00:00.000Z",
            },
            maintainers=("dave", "carol"),
        )
        reg.routes[S.npm_url("dep-n1") + "/latest"] = json.dumps(npm_version("dep-n1", "1.0.0")).encode()
        reg.npm(
            "bare-npm",
            "0.0.1",
            {"0.0.1": npm_version("bare-npm", "0.0.1", by="mallory")},
            {"0.0.1": "2023-12-22T00:00:00.000Z"},
            maintainers=("mallory",),
            created="2023-12-22T00:00:00.000Z",
        )
        reg.npm(
            "old-npm",
            "1.0.0",
            {"1.0.0": npm_version("old-npm", "1.0.0", deprecated="use new-npm instead")},
            {"1.0.0": "2015-01-01T00:00:00.000Z"},
        )
        for name in ("@tipes/util", "lodash"):
            reg.npm(name, "1.0.0", {"1.0.0": npm_version(name, "1.0.0")}, {"1.0.0": "2015-01-01T00:00:00.000Z"})


def cargo(name: str) -> dict:
    return {"name": name, "manifest": "Cargo.toml"}


def npm(name: str) -> dict:
    return {"name": name, "manifest": "web/package.json"}


def run(fx: Fixture, deps: list, source=None) -> dict:
    S.request = fx.reg
    try:
        return S.screen(deps, fx.root, NAMES, source if source is not None else A.OsvSource(S.request))
    finally:
        S.request = REAL_REQUEST


def one(fx: Fixture, item: dict, source=None) -> dict:
    return run(fx, [item], source)["packages"][0]


# --- signals ----------------------------------------------------------------


def crate_signals(fx: Fixture):
    record = one(fx, cargo("goodcrate"))
    T.check("crate: screened", record["status"] == "screened" and record["reason"] == "", str(record))
    T.check("crate: ecosystem crates.io", record["ecosystem"] == "crates.io")
    T.check("crate: version skips yanked and prerelease", record["version"] == "1.1.0", str(record["version"]))
    size = record["size"]
    T.check("crate size: download_bytes is crate_size", size["download_bytes"] == 12345, str(size))
    T.check(
        "crate size: unpacked and file count unavailable",
        size["unpacked_bytes"] == "unavailable" and size["file_count"] == "unavailable",
        str(size),
    )
    deps = record["dependencies"]
    T.check("deps: direct counts non-dev, non-optional, renamed", deps["direct"] == 4, str(deps))
    T.check("deps: transitive skips lockfile names", deps["transitive"] == 4, str(deps))
    T.check("deps: complete walk not approximate", deps["approximate"] is False, str(deps))
    T.check("deps: lockfile nearest above the manifest", deps["lockfile"] == "Cargo.lock", str(deps))
    author = record["author"]
    T.check("author: publisher of the version", author["publisher"] == "bob", str(author))
    T.check("author: owners", author["owners"] == ["alice", "bob"], str(author))
    T.check(
        "author: publisher's first release",
        author["publisher_first_release"] == "2021-03-01T00:00:00.000000+00:00",
        str(author),
    )
    T.check("author: previous publisher", author["previous_publisher"] == "alice", str(author))
    T.check("author: publisher changed", author["publisher_changed"] is True, str(author))
    est = record["establishment"]
    T.check("establishment: created", est["created"] == "2020-01-10T00:00:00.000000+00:00", str(est))
    T.check("establishment: age in days", est["age_days"] == (TODAY - datetime.date(2020, 1, 10)).days, str(est))
    T.check("establishment: releases", est["releases"] == 4, str(est))
    T.check("establishment: downloads", est["downloads"] == 5000, str(est))
    T.check("establishment: repository", est["repository"] == "https://github.com/example/crate", str(est))
    T.check("establishment: last release", est["last_release"] == "2023-01-01T00:00:00.000000+00:00", str(est))
    problems = record["problems"]
    T.check("problems: no advisories", problems["advisories"] == [], str(problems))
    T.check("problems: not yanked", problems["yanked"] is False, str(problems))
    T.check("problems: crates have no deprecated", problems["deprecated"] is None, str(problems))
    T.check("problems: not a typosquat", problems["typosquat_of"] is None, str(problems))
    T.check("evidence: a publisher change is no disqualifier", record["disqualifiers"] == [], str(record))

    record = one(fx, cargo("steady"))
    T.check("crate: no crate_size is unavailable", record["size"]["download_bytes"] == "unavailable", str(record))
    T.check("crate: no downloads is unavailable", record["establishment"]["downloads"] == "unavailable")
    T.check("crate: no repository is null", record["establishment"]["repository"] is None)
    T.check("author: same publisher is no change", record["author"]["publisher_changed"] is False, str(record))
    T.check(
        "author: first release by this publisher", record["author"]["publisher_first_release"].startswith("2019-06")
    )
    T.check("deps: none", record["dependencies"]["direct"] == 0 and record["dependencies"]["transitive"] == 0)


def walk_caps(fx: Fixture):
    record = one(fx, cargo("deep"))
    deps = record["dependencies"]
    T.check(
        "deps: depth cap counts three levels, approximate",
        deps["transitive"] == 3 and deps["approximate"] is True,
        str(deps),
    )
    saved = S.WALK_REQUESTS
    S.WALK_REQUESTS = 1
    try:
        record = one(fx, cargo("goodcrate"))
    finally:
        S.WALK_REQUESTS = saved
    deps = record["dependencies"]
    T.check("deps: request cap sets approximate", deps["approximate"] is True, str(deps))
    T.check("deps: request cap keeps the direct count", deps["direct"] == 4, str(deps))
    calls = len(fx.reg.calls)
    record = one(fx, cargo("goodcrate"))
    walked = [url for _, url in fx.reg.calls[calls:] if "index.crates.io" in url]
    T.check(
        "deps: the walk asks the index, never about lockfile names",
        not any(url.endswith(("/known-dep", "/known_dep")) for url in walked) and len(walked) == 5,
        str(walked),
    )
    (fx.root / "Cargo.lock").rename(fx.root / "Cargo.lock.away")
    try:
        record = one(fx, cargo("goodcrate"))
    finally:
        (fx.root / "Cargo.lock.away").rename(fx.root / "Cargo.lock")
    deps = record["dependencies"]
    T.check(
        "deps: no lockfile counts every package",
        deps["lockfile"] is None and deps["transitive"] == 5,
        str(deps),
    )
    (fx.root / "Cargo.lock").rename(fx.root / "Cargo.lock.away")
    (fx.root / "Cargo.lock").write_text("not = [toml\n", encoding="utf-8")
    try:
        record = one(fx, cargo("goodcrate"))
    finally:
        (fx.root / "Cargo.lock").unlink()
        (fx.root / "Cargo.lock.away").rename(fx.root / "Cargo.lock")
    T.check("deps: an unreadable lockfile is approximate", record["dependencies"]["approximate"] is True)


def npm_signals(fx: Fixture):
    record = one(fx, npm("good-npm"))
    T.check("npm: screened", record["status"] == "screened", str(record))
    T.check("npm: ecosystem npm", record["ecosystem"] == "npm")
    T.check("npm: version is dist-tags.latest", record["version"] == "2.0.0", str(record["version"]))
    size = record["size"]
    T.check("npm size: unpackedSize", size["unpacked_bytes"] == 2048, str(size))
    T.check("npm size: fileCount", size["file_count"] == 7, str(size))
    T.check("npm size: download_bytes unavailable", size["download_bytes"] == "unavailable", str(size))
    deps = record["dependencies"]
    T.check(
        "npm deps: direct, transitive past the lockfile",
        deps["direct"] == 2 and deps["transitive"] == 1 and deps["approximate"] is False,
        str(deps),
    )
    T.check("npm deps: lockfile nearest above", deps["lockfile"] == "web/pnpm-lock.yaml", str(deps))
    author = record["author"]
    T.check("npm author: publisher", author["publisher"] == "carol", str(author))
    T.check("npm author: maintainers", author["owners"] == ["carol", "dave"], str(author))
    T.check("npm author: first release", author["publisher_first_release"] == "2019-05-01T00:00:00.000Z")
    T.check(
        "npm author: previous publisher, no change",
        author["previous_publisher"] == "carol" and author["publisher_changed"] is False,
        str(author),
    )
    est = record["establishment"]
    T.check("npm establishment: created", est["created"] == "2019-04-30T00:00:00.000Z", str(est))
    T.check("npm establishment: age", est["age_days"] == (TODAY - datetime.date(2019, 4, 30)).days, str(est))
    T.check("npm establishment: releases", est["releases"] == 3, str(est))
    T.check("npm establishment: downloads unavailable", est["downloads"] == "unavailable", str(est))
    T.check(
        "npm establishment: repository",
        est["repository"] == "git+https://github.com/example/good-npm.git",
        str(est),
    )
    T.check("npm establishment: last release", est["last_release"] == "2021-07-01T00:00:00.000Z", str(est))
    T.check(
        "npm problems: not deprecated, no yanked",
        record["problems"]["deprecated"] is None and record["problems"]["yanked"] is None,
    )

    record = one(fx, npm("bare-npm"))
    size = record["size"]
    T.check(
        "npm size: absent sizes unavailable",
        size["unpacked_bytes"] == "unavailable" and size["file_count"] == "unavailable",
        str(size),
    )
    T.check(
        "npm author: a single release has no previous publisher",
        record["author"]["previous_publisher"] is None and record["author"]["publisher_changed"] is None,
        str(record["author"]),
    )
    T.check(
        "evidence: young, one release, no repository is no disqualifier",
        record["status"] == "screened"
        and record["disqualifiers"] == []
        and record["establishment"]["age_days"] == 10
        and record["establishment"]["repository"] is None,
        str(record),
    )
    record = one(fx, cargo("bigcrate"))
    T.check(
        "evidence: a large, young crate is no disqualifier",
        record["status"] == "screened" and record["disqualifiers"] == [] and record["size"]["download_bytes"] > 10**8,
        str(record),
    )


# --- disqualifiers ----------------------------------------------------------


def disqualifiers(fx: Fixture):
    record = one(fx, cargo("vulncrate"))
    T.check("advisory: disqualifier", record["disqualifiers"] == ["advisory"], str(record))
    T.check(
        "advisory: recorded with id, summary, source and aliases",
        record["problems"]["advisories"]
        == [
            {
                "id": "RUSTSEC-2099-0001",
                "summary": "Use after free in vulncrate",
                "source": "osv",
                "aliases": ["CVE-2099-1", "GHSA-aaaa-bbbb-cccc"],
            }
        ],
        str(record["problems"]),
    )
    T.check("advisory: still screened", record["status"] == "screened")
    record = one(fx, cargo("withdrawncrate"))
    T.check(
        "advisory: a withdrawn entry is ignored",
        record["disqualifiers"] == [] and record["problems"]["advisories"] == [] and record["status"] == "screened",
        str(record),
    )
    fx.reg.page = 1
    try:
        calls = len(fx.reg.queries)
        record = one(fx, cargo("pagedcrate"))
    finally:
        fx.reg.page = 0
    ids = [a["id"] for a in record["problems"]["advisories"]]
    T.check(
        "advisory: OSV pages followed",
        ids == ["GHSA-pppp", "RUSTSEC-2099-0002", "RUSTSEC-2099-0003"] and len(fx.reg.queries) - calls == 3,
        str(ids),
    )
    T.check("advisory: details used as the summary", record["problems"]["advisories"][0]["summary"].startswith("det"))
    record = one(fx, cargo("yankedcrate"))
    T.check(
        "yanked: disqualifier",
        record["disqualifiers"] == ["yanked"] and record["problems"]["yanked"] is True and record["version"] == "1.1.0",
        str(record),
    )
    record = one(fx, npm("old-npm"))
    T.check(
        "deprecated: disqualifier with the message",
        record["disqualifiers"] == ["deprecated"] and record["problems"]["deprecated"] == "use new-npm instead",
        str(record),
    )
    for item, imitated, label in (
        (cargo("tokoi"), "tokio", "distance"),
        (cargo("serdejson"), "serde_json", "delimiter"),
        (npm("@tipes/util"), "@types/*", "scope"),
    ):
        record = one(fx, item)
        T.check(
            f"typosquat by {label}: disqualifier",
            record["disqualifiers"] == ["typosquat"] and record["problems"]["typosquat_of"] == imitated,
            str(record),
        )
    for item in (cargo("serde-json"), npm("lodash")):
        record = one(fx, item)
        T.check(
            f"typosquat: listed name {item['name']} exempt",
            record["disqualifiers"] == [] and record["problems"]["typosquat_of"] is None,
            str(record),
        )
    t = S.typosquat_of
    T.check(
        "typosquat rule: two edits allowed for long names",
        t("serde-jsonn2", "crates.io", ["serde-json"]) == "serde-json",
    )
    T.check("typosquat rule: two edits too many for short names", t("tokxyo", "crates.io", ["tokio"]) is None)
    T.check("typosquat rule: a short name is not close by one substitution", t("tide", "crates.io", ["time"]) is None)
    T.check("typosquat rule: a short name is not close by one deletion", t("poem", "crates.io", ["pem"]) is None)
    T.check("typosquat rule: a short name is not close by one insertion", t("msw", "npm", ["ms"]) is None)
    T.check("typosquat rule: a short name is close by a transposition", t("tmie", "crates.io", ["time"]) == "time")
    T.check("typosquat rule: a name of five is close by one edit", t("serdr", "crates.io", ["serde"]) == "serde")
    T.check("typosquat rule: exempt names are never flagged", t("tokoi", "crates.io", ["tokio"], ["tokoi"]) is None)
    T.check("typosquat rule: exempt names are never imitated", t("tokoi", "crates.io", [], ["tokio"]) is None)
    T.check(
        "typosquat rule: a scope an exempt name uses is not flagged",
        t("@tipes/other", "npm", ["@types/node"], ["@tipes/util"]) is None,
    )
    T.check("typosquat rule: transposition is one edit", S.distance("tokio", "tokoi", 2) == 1)
    T.check(
        "typosquat rule: npm keeps _ and - apart", t("string-decoder", "npm", ["string_decoder"]) == "string_decoder"
    )
    T.check("typosquat rule: case is ignored", t("Tokio", "crates.io", ["tokio"]) is None)
    T.check("typosquat rule: an unrelated name", t("quantum-widget", "crates.io", ["tokio", "serde"]) is None)
    names = S.popular_names()
    for ecosystem, pairs in NEIGHBOURS.items():
        for name, near in pairs.items():
            T.check(
                f"typosquat list: {near} is popular and {name} is not flagged",
                near in names[ecosystem]["popular"]
                and t(name, ecosystem, names[ecosystem]["popular"], names[ecosystem]["exempt"]) is None,
            )
            T.check(
                f"typosquat rule: {name} flagged without the list's exemption",
                len(name) < 5 or t(name, ecosystem, names[ecosystem]["popular"]) == near,
            )
    record = run(fx, [cargo("vulncrate"), cargo("tokoi"), npm("old-npm"), cargo("yankedcrate")])
    T.check(
        "disqualifiers: only the four kinds",
        all(set(r["disqualifiers"]) <= set(S.DISQUALIFIERS) for r in record["packages"]),
    )


def unscreened(fx: Fixture):
    for item, label in (
        (cargo("flaky"), "a failed crates.io fetch"),
        (cargo("missing"), "a crate not on the registry"),
        (npm("missing-npm"), "an npm package not on the registry"),
        ({"name": "requests", "manifest": "docs/requirements.txt"}, "an unsupported manifest"),
        ({"name": "x", "manifest": "../Cargo.toml"}, "a manifest outside the root"),
        (cargo("bad name"), "an invalid name"),
        (cargo("brokenvuln"), "a failed advisory detail"),
    ):
        record = one(fx, item)
        T.check(
            f"unscreened: {label}",
            record["status"] == "unscreened" and record["reason"] and record["status"] != "screened",
            str(record),
        )
    T.check(
        "unscreened: the manifest reason says so",
        one(fx, {"name": "requests", "manifest": "docs/requirements.txt"})["reason"] == "unsupported manifest",
    )
    fx.reg.osv_error = S.ScreenError("HTTP 500")
    try:
        record = run(fx, [cargo("goodcrate"), npm("old-npm"), cargo("tokoi")])
    finally:
        fx.reg.osv_error = None
    statuses = [(r["status"], r["reason"].startswith("advisory query failed")) for r in record["packages"]]
    T.check(
        "unscreened: an OSV error leaves no package screened", statuses == [("unscreened", True)] * 3, str(statuses)
    )
    T.check(
        "unscreened: disqualifiers found before the failure stay listed",
        record["packages"][1]["disqualifiers"] == ["deprecated"]
        and record["packages"][2]["disqualifiers"] == ["typosquat"],
        str(record["packages"]),
    )
    record = run(fx, [cargo("goodcrate"), cargo("brokenvuln")])
    T.check(
        "unscreened: a failed advisory detail affects only its package",
        [r["status"] for r in record["packages"]] == ["screened", "unscreened"],
    )

    class Broken:
        name = "broken"

        def query(self, packages):
            raise RuntimeError("boom")

    record = one(fx, cargo("goodcrate"), Broken())
    T.check("unscreened: any source exception fails closed", record["status"] == "unscreened", str(record))

    class Partial:
        name = "partial"

        def query(self, packages):
            return {}

    record = one(fx, cargo("goodcrate"), Partial())
    T.check("unscreened: a package the source did not answer", record["status"] == "unscreened", str(record))


def advisory_source(fx: Fixture):
    class Fake:
        name = "fake"

        def __init__(self):
            self.asked = []

        def query(self, packages):
            self.asked.extend(packages)
            return {key: [A.Advisory("FAKE-1", "fake problem", "fake", ())] for key in packages}

    fake = Fake()
    calls = len(fx.reg.queries)
    result = run(fx, [cargo("goodcrate"), npm("good-npm")], fake)
    T.check(
        "source: a fake AdvisorySource swaps in for OsvSource",
        all(r["disqualifiers"] == ["advisory"] for r in result["packages"]) and result["advisory_source"] == "fake",
        str(result),
    )
    T.check(
        "source: asked by (ecosystem, name, version)",
        fake.asked == [("crates.io", "goodcrate", "1.1.0"), ("npm", "good-npm", "2.0.0")],
        str(fake.asked),
    )
    T.check("source: OSV not reached with another source", len(fx.reg.queries) == calls)
    run(fx, [cargo("goodcrate"), npm("good-npm")])
    sent = fx.reg.queries[-1]
    T.check(
        "osv: ecosystems crates.io and npm, the exact version",
        [(q["package"]["ecosystem"], q["package"]["name"], q["version"]) for q in sent]
        == [("crates.io", "goodcrate", "1.1.0"), ("npm", "good-npm", "2.0.0")],
        str(sent),
    )
    osv = A.OsvSource(lambda *a: b"{}")
    try:
        osv.query([("PyPI", "x", "1")])
        refused = False
    except A.AdvisoryError:
        refused = True
    T.check("osv: other ecosystems refused", refused)
    chunks = []

    def counting(method, url, cap, body=None):
        queries = json.loads(body)["queries"]
        chunks.append(len(queries))
        return json.dumps({"results": [{} for _ in queries]}).encode()

    keys = [("npm", f"p{i}", "1.0.0") for i in range(1500)]
    answer = A.OsvSource(counting).query(keys)
    T.check("osv: querybatch in chunks of 1000", chunks == [1000, 500], str(chunks))
    T.check("osv: every package answered", len(answer) == 1500 and all(v == [] for v in answer.values()))
    bad = A.OsvSource(lambda *a: json.dumps({"results": []}).encode())
    try:
        bad.query([("npm", "x", "1.0.0")])
        failed = False
    except A.AdvisoryError:
        failed = True
    T.check("osv: a short results list fails", failed)


# --- transport --------------------------------------------------------------


def refused(call) -> bool:
    try:
        call()
    except S.ScreenError:
        return True
    return False


def transport():
    for url in (
        "http://registry.npmjs.org/x",
        "https://evil.example/x",
        "https://user:pw@api.osv.dev/v1/querybatch",
        "https://api.osv.dev:8443/v1/querybatch",
        "https://api.osv.dev.evil.example/v1/querybatch",
        "file:///etc/passwd",
    ):
        T.check(f"transport: refuses {url}", refused(lambda: S.check_url(url)))
    for host in ("crates.io", "index.crates.io", "static.crates.io", "registry.npmjs.org", "api.osv.dev"):
        T.check(f"transport: allows {host}", S.check_url(f"https://{host}/x") == host)
    T.check("transport: exactly five hosts", len(S.HOSTS) == 5, str(sorted(S.HOSTS)))
    before = len(BLOCKED)
    T.check(
        "transport: POST elsewhere refused before any socket",
        refused(lambda: REAL_REQUEST("POST", "https://api.osv.dev/v1/query", 100, b"{}"))
        and refused(lambda: REAL_REQUEST("POST", "https://registry.npmjs.org/x", 100, b"{}")),
    )
    T.check("transport: other methods refused", refused(lambda: REAL_REQUEST("PUT", A.QUERYBATCH, 100, b"{}")))
    T.check("transport: GET with a body refused", refused(lambda: REAL_REQUEST("GET", A.QUERYBATCH, 100, b"{}")))
    T.check("transport: refusals opened no socket", len(BLOCKED) == before)
    T.check(
        "transport: a real GET with sockets blocked fails",
        refused(lambda: REAL_REQUEST("GET", "https://registry.npmjs.org/x", 100)),
    )
    T.check(
        "transport: a real querybatch POST with sockets blocked fails",
        refused(lambda: REAL_REQUEST("POST", A.QUERYBATCH, 100, b"{}")),
    )
    T.check("transport: the socket block was hit", len(BLOCKED) > before)

    saved = os.environ.get("HTTPS_PROXY")
    os.environ["HTTPS_PROXY"] = "http://proxy.example:3128"
    try:
        built = S.P.opener()
    finally:
        if saved is None:
            os.environ.pop("HTTPS_PROXY")
        else:
            os.environ["HTTPS_PROXY"] = saved
    proxies = [h.proxies for h in built.handlers if isinstance(h, urllib.request.ProxyHandler)]
    T.check("transport: no proxy even with HTTPS_PROXY", all(p == {} for p in proxies), str(proxies))
    proxy_opens = [h for h in built.handlers if hasattr(h, "proxy_open") and getattr(h, "proxies", None)]
    T.check("transport: no handler opens through a proxy", not proxy_opens, str(proxy_opens))
    redirects = [type(h) for h in built.handlers if isinstance(h, urllib.request.HTTPRedirectHandler)]
    T.check("transport: only the refusing redirect handler", redirects == [S.P.Refuse], str(redirects))
    T.check(
        "transport: a redirect is refused",
        S.P.Refuse().redirect_request(S.build("GET", "https://crates.io/x"), None, 302, "Found", {}, "https://e/")
        is None,
    )

    class Response(io.BytesIO):
        pass

    seen, sleeps = [], []

    class Opener:
        def __init__(self, body: bytes):
            self.body = body

        def open(self, request, timeout):
            seen.append(request)
            return Response(self.body)

    saved_opener, saved_sleep, saved_delay = S.P.opener, S.time.sleep, S.RATE_DELAY
    try:
        S.P.opener = lambda: Opener(b"x" * 100)
        S.time.sleep = sleeps.append
        S.RATE_DELAY = 1.0
        T.check("transport: a body at its cap", REAL_REQUEST("GET", "https://crates.io/x", 100) == b"x" * 100)
        T.check("transport: the crates.io pause", sleeps == [1.0], str(sleeps))
        T.check(
            "transport: over the cap fails", refused(lambda: REAL_REQUEST("GET", "https://registry.npmjs.org/x", 99))
        )
        REAL_REQUEST("POST", A.QUERYBATCH, 100, b"{}")
        T.check("transport: no pause for OSV or npm", sleeps == [1.0], str(sleeps))
    finally:
        S.P.opener, S.time.sleep, S.RATE_DELAY = saved_opener, saved_sleep, saved_delay
    post = seen[-1]
    headers = {k.lower() for k, _ in post.header_items()}
    T.check("transport: POST method and body", post.get_method() == "POST" and post.data == b"{}")
    T.check(
        "transport: no Authorization header",
        headers == {"user-agent", "accept", "content-type"}
        and all("authorization" not in {k.lower() for k, _ in r.header_items()} for r in seen),
        str(headers),
    )
    T.check("transport: GET carries no body", seen[0].get_method() == "GET" and seen[0].data is None)


# --- the command line -------------------------------------------------------


PLAN = """# Plan: x

## Steps
1. Something.

## Footprint

```json
{{
  "footprint": {{
    "files": ["Cargo.toml"],
    "new_files": [],
    "estimated_lines": 1,
    "new_dependencies": {deps},
    "external_steps": [],
    "gate_changes": [],
    "overridden_assumptions": []
  }}
}}
```

## Verification
"""


def cli(fx: Fixture, tmp: Path):
    plan, out = tmp / "plan.md", tmp / "screen.json"
    deps = [cargo("goodcrate"), npm("old-npm"), {"name": "requests", "manifest": "docs/requirements.txt"}]
    plan.write_text(PLAN.format(deps=json.dumps(deps)), encoding="utf-8")
    argv = ["plan", "--plan", str(plan), "--root", str(fx.root), "--out", str(out)]
    S.request = fx.reg
    try:
        outputs = []
        for _ in range(2):
            with contextlib.redirect_stdout(io.StringIO()) as stdout:
                code = S.main(argv)
            T.check("cli: exit 0", code == 0, stdout.getvalue())
            outputs.append(out.read_bytes())
        record = json.loads(outputs[0])
        T.check("cli: the same output on two runs", outputs[0] == outputs[1])
        T.check(
            "cli: the record shape",
            record["tool"] == "dependency-screen"
            and record["version"] == 1
            and record["mode"] == "plan"
            and record["date"] == "2024-01-01"
            and record["advisory_source"] == "osv",
            str({k: v for k, v in record.items() if k != "packages"}),
        )
        T.check(
            "cli: one record per declared package, in order",
            [(p["name"], p["status"]) for p in record["packages"]]
            == [("goodcrate", "screened"), ("old-npm", "screened"), ("requests", "unscreened")],
            str(record["packages"]),
        )
        T.check("cli: the default source is OSV", any(m == "POST" for m, _ in fx.reg.calls))
        plan.write_text(PLAN.format(deps="[]"), encoding="utf-8")
        with contextlib.redirect_stdout(io.StringIO()):
            code = S.main(argv)
        T.check(
            "cli: no declared packages, exit 0 and empty", code == 0 and json.loads(out.read_text())["packages"] == []
        )
        for label, text in (
            ("no footprint", "# Plan\n"),
            ("not JSON", "## Footprint\n\n```json\n{\n```\n"),
            ("unclosed fence", "## Footprint\n\n```json\n{}\n"),
            ("an entry with a version", PLAN.format(deps=json.dumps([dict(cargo("x"), version="1")]))),
        ):
            plan.write_text(text, encoding="utf-8")
            with contextlib.redirect_stderr(io.StringIO()):
                T.check(f"cli: {label}: exit 2", S.main(argv) == 2)
        plan.write_text(PLAN.format(deps="[]"), encoding="utf-8")
        for label, bad in (
            ("no arguments", []),
            ("unknown command", ["review", "--plan", str(plan), "--root", str(fx.root), "--out", str(out)]),
            ("no --out", ["plan", "--plan", str(plan), "--root", str(fx.root)]),
            ("unknown option", argv + ["--bogus"]),
            ("missing plan", ["plan", "--plan", str(tmp / "nope.md"), "--root", str(fx.root), "--out", str(out)]),
            ("root not a directory", ["plan", "--plan", str(plan), "--root", str(plan), "--out", str(out)]),
            ("unwritable output", ["plan", "--plan", str(plan), "--root", str(fx.root), "--out", str(tmp / "no/x")]),
        ):
            with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
                T.check(f"cli: {label}: exit 2", S.main(bad) == 2)
    finally:
        S.request = REAL_REQUEST


# --- the review mode --------------------------------------------------------


REPO = "https://github.com/example/locked"


def ago(days: int) -> str:
    return (TODAY - datetime.timedelta(days=days)).isoformat() + "T00:00:00Z"


def locked(ecosystem: str, name: str, version: str = "1.0.0", direct: bool = False, registry: bool = True) -> dict:
    """A packages.json entry as new-packages.py writes it."""
    return {
        "ecosystem": ecosystem,
        "lockfile": "Cargo.lock" if ecosystem == "cargo" else "web/pnpm-lock.yaml",
        "name": name,
        "version": version,
        "previous": [],
        "source": "registry+https://github.com/rust-lang/crates.io-index" if ecosystem == "cargo" else "registry",
        "registry": registry,
        "direct": direct,
        "checksum": "0" * 64 if ecosystem == "cargo" else None,
        "integrity": "sha512-AA==" if ecosystem == "npm" else None,
    }


def crate_meta(
    created=None, published=None, by="alice", previous="alice", downloads=50000, repository=REPO, yanked=False
) -> dict:
    """The registry part of a crate's metadata.json as prefetch.py trims it; previous None is a first release."""
    return {
        "crate": {
            "created_at": created or ago(2000),
            "updated_at": None,
            "downloads": downloads,
            "recent_downloads": None,
            "repository": repository,
            "homepage": None,
        },
        "version": {
            "created_at": published or ago(200),
            "yanked": yanked,
            "license": "MIT",
            "downloads": 1,
            "rust_version": None,
            "published_by": by,
        },
        "owners": [{"login": "alice", "kind": "user"}],
        "versions_count": 2,
        "history": [],
        "previous": (None if previous is None else {"num": "0.9.0", "created_at": ago(2000), "published_by": previous}),
    }


def npm_meta(
    created=None, published=None, by="carol", previous="carol", repository=REPO, deprecated=None, source="packument"
) -> dict:
    """The registry part of an npm package's metadata.json as prefetch.py trims it."""
    return {
        "source": source,
        "created": created or ago(2000),
        "modified": None,
        "versions_count": 2,
        "history": [],
        "time": published or ago(200),
        "previous": None if previous is None else {"version": "0.9.0", "time": ago(2000), "publisher": previous},
        "maintainers": ["carol"],
        "publisher": by,
        "license": "MIT",
        "repository": repository,
        "deprecated": deprecated,
        "scripts": {},
        "hasInstallScript": None,
        "downloads": "unavailable",
    }


class Locked:
    """A dependencies directory with packages.json, prefetch.json and each fetched package's metadata.json."""

    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.counter = 0

    def make(self, items: list) -> Path:
        """items: (entry, registry metadata), with metadata a dict, ("refused" or "failed", reason), or "missing"."""
        self.counter += 1
        out = self.tmp / f"review-{self.counter}"
        out.mkdir()
        records = []
        for entry, metadata in items:
            record = {k: entry[k] for k in ("ecosystem", "lockfile", "name", "version")}
            record.update(status="fetched", reason="", dir=None)
            if isinstance(metadata, tuple):
                record.update(status=metadata[0], reason=metadata[1])
            else:
                record["dir"] = f"{entry['ecosystem']}/{entry['name']}@{entry['version']}"
                if isinstance(metadata, dict):
                    (out / record["dir"]).mkdir(parents=True)
                    document = {"registry": metadata, "build_time": "none", "found": [], "files": []}
                    (out / record["dir"] / "metadata.json").write_text(json.dumps(document), encoding="utf-8")
            records.append(record)
        packages = {"base": "b", "head": "h", "changed": [], "problems": [], "packages": [e for e, _ in items]}
        (out / "packages.json").write_text(json.dumps(packages), encoding="utf-8")
        (out / "prefetch.json").write_text(json.dumps({"packages": records}), encoding="utf-8")
        return out


def reviewed(fx: Fixture, out: Path) -> tuple:
    """(exit, stdout, screen.json or None, requests made) after `screen.py review --dir out`."""
    calls = len(fx.reg.calls)
    S.request = fx.reg
    try:
        with contextlib.redirect_stdout(io.StringIO()) as stdout, contextlib.redirect_stderr(io.StringIO()):
            code = S.main(["review", "--dir", str(out)])
    finally:
        S.request = REAL_REQUEST
    target = out / "screen.json"
    record = json.loads(target.read_text(encoding="utf-8")) if target.exists() else None
    return code, stdout.getvalue(), record, fx.reg.calls[calls:]


def screened(fx: Fixture, box: Locked, items: list) -> list:
    """The review records of items, screened against the self-test's typosquat list."""
    out = box.make(items)
    S.request = fx.reg
    try:
        packages = S.P.load(out / "packages.json")
        record = S.review(packages, S.prefetched(out, packages), out, NAMES, A.OsvSource(S.request))
    finally:
        S.request = REAL_REQUEST
    return record["packages"]


def flags_of(fx: Fixture, box: Locked, entry: dict, metadata) -> list:
    return screened(fx, box, [(entry, metadata)])[0]["flags"]


def review_mode(fx: Fixture, tmp: Path):
    box = Locked(tmp)
    fx.reg.osv[("crates.io", "vulnlocked", "1.0.0")] = ["RUSTSEC-2099-0001"]
    items = [
        (locked("cargo", "cleancrate", direct=True), crate_meta()),
        (locked("cargo", "deepcrate"), crate_meta()),
        (locked("npm", "clean-npm"), npm_meta()),
        (locked("cargo", "vulnlocked"), crate_meta()),
    ]
    records = screened(fx, box, items)
    T.check(
        "review: one record per lockfile package, transitive ones included",
        [(r["name"], r["direct"]) for r in records]
        == [("cleancrate", True), ("deepcrate", False), ("clean-npm", False), ("vulnlocked", False)],
        str(records),
    )
    T.check(
        "review: the record keys",
        all(
            set(r)
            == {
                "name",
                "ecosystem",
                "lockfile",
                "version",
                "direct",
                "status",
                "reason",
                "signals",
                "problems",
                "disqualifiers",
                "flags",
                "flagged",
            }
            for r in records
        ),
        str(records[0]),
    )
    T.check(
        "review: cargo is crates.io, the locked version kept",
        [(r["ecosystem"], r["version"]) for r in records[:3]]
        == [("crates.io", "1.0.0"), ("crates.io", "1.0.0"), ("npm", "1.0.0")],
    )
    clean = records[0]
    T.check(
        "review: a clean package is screened and unflagged",
        clean["status"] == "screened" and clean["flags"] == [] and clean["flagged"] is False,
        str(clean),
    )
    T.check(
        "review: the signals of a crate",
        clean["signals"]
        == {
            "age_days": 2000,
            "version_age_days": 200,
            "publisher": "alice",
            "previous_publisher": "alice",
            "publisher_changed": False,
            "downloads": 50000,
            "repository": REPO,
        },
        str(clean["signals"]),
    )
    T.check(
        "review: the signals of an npm package",
        records[2]["signals"]
        == {
            "age_days": 2000,
            "version_age_days": 200,
            "publisher": "carol",
            "previous_publisher": "carol",
            "publisher_changed": False,
            "downloads": "unavailable",
            "repository": REPO,
        }
        and records[2]["flagged"] is False,
        str(records[2]),
    )
    vuln = records[3]
    T.check(
        "review disqualifier: an advisory on a transitive crate",
        vuln["disqualifiers"] == ["advisory"]
        and vuln["flags"] == ["advisory"]
        and vuln["flagged"] is True
        and vuln["problems"]["advisories"][0]["id"] == "RUSTSEC-2099-0001",
        str(vuln),
    )
    for entry, metadata, wanted, label in (
        (locked("cargo", "yankedlock"), crate_meta(yanked=True), ["yanked"], "a yanked crate"),
        (locked("npm", "gone-npm"), npm_meta(deprecated="use other"), ["deprecated"], "a deprecated npm package"),
        (locked("cargo", "tokoi"), crate_meta(), ["typosquat"], "a typosquat"),
    ):
        T.check(f"review disqualifier: {label}, transitive", flags_of(fx, box, entry, metadata) == wanted)
    record = screened(fx, box, [(locked("npm", "gone-npm"), npm_meta(deprecated="use other"))])[0]
    T.check("review: the deprecation message kept", record["problems"]["deprecated"] == "use other", str(record))

    crate = locked("cargo", "edgecrate")
    for label, under, at, flag in (
        ("a new package", {"created": ago(89)}, {"created": ago(90)}, "new_package"),
        ("a new version", {"published": ago(13)}, {"published": ago(14)}, "new_version"),
        ("low crates.io downloads", {"downloads": 9999}, {"downloads": 10000}, "low_downloads"),
    ):
        T.check(
            f"review threshold: {label} just under the limit flags",
            flags_of(fx, box, crate, crate_meta(**under)) == [flag],
        )
        T.check(f"review threshold: {label} at the limit does not", flags_of(fx, box, crate, crate_meta(**at)) == [])
    T.check(
        "review threshold: the limits are the constants",
        (S.NEW_PACKAGE_DAYS, S.NEW_VERSION_DAYS, S.LOW_DOWNLOADS) == (90, 14, 10000),
    )
    T.check(
        "review threshold: a publisher change flags",
        flags_of(fx, box, crate, crate_meta(by="mallory", previous="alice")) == ["publisher_changed"],
    )
    T.check("review threshold: the same publisher does not", flags_of(fx, box, crate, crate_meta()) == [])
    T.check(
        "review threshold: an npm publisher change flags",
        flags_of(fx, box, locked("npm", "moved-npm"), npm_meta(by="mallory")) == ["publisher_changed"],
    )
    T.check(
        "review threshold: a first release has no publisher change",
        screened(fx, box, [(crate, crate_meta(previous=None))])[0]["signals"]["publisher_changed"] is None
        and flags_of(fx, box, crate, crate_meta(previous=None)) == [],
    )
    T.check(
        "review threshold: no repository flags",
        flags_of(fx, box, crate, crate_meta(repository=None)) == ["no_repository"],
    )
    T.check(
        "review threshold: no npm repository flags",
        flags_of(fx, box, locked("npm", "bare-lock"), npm_meta(repository=None)) == ["no_repository"],
    )
    T.check("review threshold: a repository link does not", flags_of(fx, box, crate, crate_meta()) == [])
    T.check(
        "review: every flag at once, in order",
        flags_of(
            fx,
            box,
            locked("cargo", "tokoi"),
            crate_meta(created=ago(1), published=ago(1), by="x", downloads=1, repository=None, yanked=True),
        )
        == ["yanked", "typosquat", "new_package", "new_version", "publisher_changed", "low_downloads", "no_repository"],
    )

    blank = crate_meta()
    blank["crate"] = {"created_at": None, "downloads": None}
    blank["version"] = {"created_at": "not a date", "yanked": False, "published_by": None}
    del blank["previous"]
    record = screened(fx, box, [(crate, blank)])[0]
    T.check(
        "review: unavailable values never flag",
        record["status"] == "screened"
        and record["flags"] == []
        and all(v == "unavailable" for v in record["signals"].values()),
        str(record),
    )
    record = screened(
        fx, box, [(locked("npm", "doc-npm"), npm_meta(published=None, previous=None, source="version document"))]
    )[0]
    T.check(
        "review: an npm version document gives no previous publisher, and no flag",
        record["signals"]["previous_publisher"] == "unavailable"
        and record["signals"]["publisher_changed"] == "unavailable"
        and record["flags"] == [],
        str(record),
    )
    T.check(
        "review: npm downloads never flag",
        screened(fx, box, [(locked("npm", "clean-npm"), dict(npm_meta(), downloads=0))])[0]["flags"] == [],
    )

    records = screened(
        fx,
        box,
        [
            (locked("cargo", "gitdep", registry=False), ("refused", "not from the public registry: git+https://x")),
            (locked("npm", "broken-npm"), ("failed", "https://registry.npmjs.org/broken-npm: HTTP 500")),
            (locked("cargo", "lostcrate"), "missing"),
            (locked("cargo", "tokoi"), ("failed", "HTTP 500")),
        ],
    )
    for record, label in zip(records, ("a refused prefetch", "a failed prefetch", "a missing metadata.json")):
        T.check(
            f"review unscreened: {label}",
            record["status"] == "unscreened"
            and record["reason"]
            and record["flags"] == ["unscreened"]
            and record["flagged"] is True,
            str(record),
        )
    T.check(
        "review unscreened: the prefetch reason kept", "public registry" in records[0]["reason"], records[0]["reason"]
    )
    T.check(
        "review unscreened: a typosquat still listed",
        records[3]["flags"] == ["typosquat", "unscreened"],
        str(records[3]),
    )
    fx.reg.osv_error = S.ScreenError("HTTP 500")
    try:
        records = screened(fx, box, [(locked("cargo", "cleancrate"), crate_meta())])
    finally:
        fx.reg.osv_error = None
    T.check(
        "review unscreened: an OSV error",
        records[0]["status"] == "unscreened"
        and records[0]["reason"].startswith("advisory query failed")
        and records[0]["flags"] == ["unscreened"],
        str(records),
    )

    # the command line
    out = box.make(items + [(locked("npm", "broken-npm"), ("failed", "HTTP 500"))])
    (out / "screen.json").write_text("stale\n", encoding="utf-8")
    code, stdout, record, calls = reviewed(fx, out)
    T.check("review cli: exit 0", code == 0, stdout)
    T.check(
        "review cli: the record shape",
        record is not None
        and {k: v for k, v in record.items() if k != "packages"}
        == {"tool": "dependency-screen", "version": 1, "mode": "review", "date": "2024-01-01", "advisory_source": "osv"}
        and len(record["packages"]) == 5,
        str(record)[:500],
    )
    T.check(
        "review cli: the summary",
        stdout.strip() == f"4 screened, 1 unscreened, 2 flagged: {out / 'screen.json'}",
        stdout,
    )
    T.check(
        "review transport: only api.osv.dev, over HTTPS",
        calls and all(m in ("GET", "POST") and u.startswith("https://api.osv.dev/") for m, u in calls),
        str(calls),
    )
    T.check(
        "review transport: no registry host",
        not any(urllib.parse.urlsplit(u).netloc in S.P.HOSTS for _, u in calls),
        str(calls),
    )
    first = (out / "screen.json").read_bytes()
    reviewed(fx, out)
    T.check("review cli: the same output on two runs", (out / "screen.json").read_bytes() == first)

    empty = tmp / "review-empty"
    empty.mkdir()
    (empty / "screen.json").write_text("stale\n", encoding="utf-8")
    code, stdout, record, calls = reviewed(fx, empty)
    T.check(
        "review cli: no packages.json, exit 0 and no screen.json",
        code == 0 and stdout.strip() == "no packages.json" and record is None and not calls,
        stdout,
    )
    for label, name, text in (
        ("packages.json not JSON", "packages.json", "{"),
        ("packages.json in the wrong shape", "packages.json", json.dumps({"packages": [{"name": "x"}]})),
        ("prefetch.json not JSON", "prefetch.json", "{"),
        ("prefetch.json for other packages", "prefetch.json", json.dumps({"packages": []})),
    ):
        out = box.make(items[:1])
        (out / "screen.json").write_text("stale\n", encoding="utf-8")
        (out / name).write_text(text, encoding="utf-8")
        code, _, record, _ = reviewed(fx, out)
        T.check(f"review cli: {label}: exit 2, stale screen.json removed", code == 2 and record is None)
    out = box.make(items[:1])
    (out / "prefetch.json").unlink()
    T.check("review cli: no prefetch.json: exit 2", reviewed(fx, out)[0] == 2)
    for label, bad in (
        ("review without --dir", ["review"]),
        ("review with plan options", ["review", "--dir", str(out), "--plan", "x"]),
    ):
        with contextlib.redirect_stderr(io.StringIO()):
            T.check(f"cli: {label}: exit 2", S.main(bad) == 2)


# --- static rules -----------------------------------------------------------


def names_list():
    data = json.loads((HERE / "popular-names.json").read_text(encoding="utf-8"))
    T.check("names: note, popular and exempt only", set(data) == {"note", "popular", "exempt"}, str(sorted(data)))
    T.check("names: the note says how it was built", isinstance(data.get("note"), str) and "lock" in data["note"])
    for kind, least in (("popular", 300), ("exempt", 1)):
        group = data.get(kind) if isinstance(data.get(kind), dict) else {}
        T.check(f"names: {kind} has crates.io and npm only", set(group) == {"crates.io", "npm"}, str(sorted(group)))
        for ecosystem in ("crates.io", "npm"):
            names = group.get(ecosystem, [])
            T.check(
                f"names: {kind} {ecosystem} sorted and de-duplicated",
                names == sorted(set(names)) and len(names) >= least,
                str(len(names)),
            )
    for ecosystem in ("crates.io", "npm"):
        both = set(data["popular"][ecosystem]) & set(data["exempt"][ecosystem])
        T.check(f"names: {ecosystem} popular and exempt do not overlap", not both, str(sorted(both)))
        for name in NEIGHBOURS[ecosystem]:
            T.check(f"names: {name} is exempt", name in data["exempt"][ecosystem])
    loaded = S.popular_names()
    T.check(
        "names: loaded by the screen",
        all(loaded[e] == {k: data[k][e] for k in ("popular", "exempt")} for e in ("crates.io", "npm")),
    )


def inside(tree: ast.AST) -> dict:
    """Each node of tree mapped to the name of its outermost enclosing function, or ""."""
    owner = {}

    def visit(node, function):
        owner[node] = function
        for child in ast.iter_child_nodes(node):
            name = function or (child.name if isinstance(child, ast.FunctionDef) else "")
            visit(child, name)

    visit(tree, "")
    return owner


def network_names(tree: ast.AST) -> list:
    found = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute) and node.attr in NETWORK:
            found.append(node)
        elif isinstance(node, ast.Name) and node.id in NETWORK:
            found.append(node)
    return found


def yq_argv(node: ast.AST) -> bool:
    """Whether node is a subprocess.run call whose argv is a literal list starting with "yq"."""
    if not node.args or not isinstance(node.args[0], ast.List) or not node.args[0].elts:
        return False
    first = node.args[0].elts[0]
    return isinstance(first, ast.Constant) and first.value == "yq"


def side_doors(tree: ast.AST, owner: dict):
    """The rules that close screen.py's ways round request: subprocess for yq only, no dynamic lookup outside load."""
    aliased = []
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and (node.module or "").split(".")[0] in {"subprocess", "importlib"}:
            aliased.append(f"from {node.module}")
        elif isinstance(node, ast.Import):
            aliased.extend(
                a.name for a in node.names if a.asname and a.name.split(".")[0] in {"subprocess", "importlib"}
            )
    T.check("ast: subprocess and importlib imported only by their own names", not aliased, str(aliased))
    parents = {child: node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    runs, stray = [], []
    for node in ast.walk(tree):
        if isinstance(node, ast.Name) and node.id == "subprocess":
            parent = parents.get(node)
            attr = parent.attr if isinstance(parent, ast.Attribute) else None
            if attr == "run" and isinstance(parents.get(parent), ast.Call) and parents[parent].func is parent:
                runs.append(parents[parent])
            elif attr != "SubprocessError":
                stray.append(f"{owner[node] or '<module>'}:{node.lineno}")
    T.check("ast: subprocess is used only for run and SubprocessError", not stray, str(stray))
    T.check("ast: screen.py runs a subprocess", bool(runs))
    bad = [f"{owner[n] or '<module>'}:{n.lineno}" for n in runs if not yq_argv(n)]
    T.check('ast: every subprocess.run has a literal list argv starting with "yq"', not bad, str(bad))
    dynamic = sorted(
        f"{owner[n] or '<module>'}:{n.lineno}"
        for n in ast.walk(tree)
        if isinstance(n, ast.Name) and n.id in {"getattr", "__import__", "importlib"} and owner[n] != "load"
    )
    T.check("ast: getattr, __import__ and importlib only in load", not dynamic, str(dynamic))


def source_rules():
    tree = ast.parse(SCRIPT.read_text(encoding="utf-8"), feature_version=(3, 9))
    owner = inside(tree)
    outside = sorted({owner[n] or "<module>" for n in network_names(tree) if owner[n] != "request"})
    T.check("ast: request is the only function in screen.py that opens a URL", not outside, str(outside))
    T.check("ast: request opens through the opener", any(owner[n] == "request" for n in network_names(tree)))
    side_doors(tree, owner)
    tree = ast.parse(ADVISORIES.read_text(encoding="utf-8"), feature_version=(3, 9))
    imports = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            imports.add(node.module or "")
    T.check(
        "ast: advisories.py imports nothing that opens a connection",
        not imports & {"urllib.request", "socket", "http", "http.client", "ssl", "urllib3"},
        str(sorted(imports)),
    )
    T.check("ast: advisories.py opens nothing", not network_names(tree))
    for path in (SCRIPT, ADVISORIES, Path(__file__).resolve()):
        tree = ast.parse(path.read_text(encoding="utf-8"), feature_version=(3, 9))
        modules = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                modules.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                modules.add((node.module or "").split(".")[0])
        T.check(f"{path.name}: Python 3.9 syntax, standard library only", modules <= STDLIB, str(modules - STDLIB))


def main() -> int:
    if shutil.which("yq") is None:
        print("FAIL yq is not on PATH")
        return 1
    tmp = Path(tempfile.mkdtemp(prefix="msw-screen-"))
    try:
        fx = Fixture(tmp)
        crate_signals(fx)
        walk_caps(fx)
        npm_signals(fx)
        disqualifiers(fx)
        unscreened(fx)
        advisory_source(fx)
        transport()
        cli(fx, tmp)
        review_mode(fx, tmp)
        names_list()
        source_rules()
    finally:
        S.request = REAL_REQUEST
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
