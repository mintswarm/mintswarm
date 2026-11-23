#!/usr/bin/env python3
"""Fetch registry metadata and package source for the review stage's dependency verdict.

    prefetch.py --dir <dir>

Run by the dispatching session after new-packages.py, with network access to
the registry hosts only. It first removes `<dir>/prefetch.json`, `<dir>/cargo`
and `<dir>/npm` left by an earlier run (a symlink is unlinked, never
followed). With no `<dir>/packages.json` it prints `no packages.json` and
exits 0.

For every package in `packages.json` it contacts only `crates.io`,
`index.crates.io`, `static.crates.io` and `registry.npmjs.org`, over HTTPS,
with no proxy, no redirect and no credential, through `fetch`, the only
network path. A package that is not from the public registry, has a name or
version outside the registry's rules, or has no lockfile hash is `refused`
with no request. A crate archive must match the Cargo.lock `checksum`
(SHA-256), an npm tarball the pnpm `integrity` (sha512 only); any fetch
error, mismatch, unsafe archive or oversized response makes the package
`failed`. Otherwise it is `fetched`.

Archives are gunzipped with a bounded stream and read member by member in
memory: an absolute member, a `..`, `.` or empty component, a backslash, a
link, anything but a file or a directory, an oversized member, too many
members or a member outside the single top directory fails the archive as a
whole. Nothing is extracted to disk by tarfile and nothing from the package
is executed; the crate's Cargo.toml is parsed by `yq -p=toml` on stdin, the
only subprocess.

For a fetched package it writes `<dir>/<ecosystem>/<name>@<version>/`
holding `metadata.json` (`registry`: the trimmed registry metadata;
`build_time`: `none`, the list of build or install code found, or
`undetermined`; `files`: what was copied) and `files/1.txt`, `files/2.txt`,
… (the manifest and the build or install code, mode 0644). `<dir>/prefetch.json`
lists every package with its `status`, `reason` and `dir`.

Standard library only, apart from running yq. Exit 0 whatever happens to
single packages; 2 on bad arguments, a malformed packages.json or an output
that cannot be written.
"""

import argparse
import base64
import hashlib
import io
import json
import re
import shutil
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from pathlib import Path

HOSTS = frozenset({"crates.io", "index.crates.io", "static.crates.io", "registry.npmjs.org"})
CRATES_HOSTS = frozenset({"crates.io", "index.crates.io", "static.crates.io"})
NPM_TARBALLS = "https://registry.npmjs.org/"
USER_AGENT = "mintswarm-dependency-prefetch"
TIMEOUT = 30
RATE_DELAY = 1.0
MIB = 1024 * 1024
CRATES_META_CAP = 32 * MIB
NPM_META_CAP = 64 * MIB
ARCHIVE_CAP = 32 * MIB
TOTAL_CAP = 128 * MIB
MEMBER_CAP = 32 * MIB
MEMBER_COUNT = 20000
COPY_CAP = 1 * MIB
HISTORY = 20
TEXT_CAP = 500
SCRIPT_CAP = 1000
CRATE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}")
NPM_NAME = re.compile(r"(@[a-z0-9~-][a-z0-9._~-]*/)?[a-z0-9~-][a-z0-9._~-]*")
VERSION = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+-]{0,63}")
INSTALL_SCRIPTS = ("preinstall", "install", "postinstall")
CRATE_FIELDS = ("created_at", "updated_at", "downloads", "recent_downloads", "repository", "homepage")
VERSION_FIELDS = ("created_at", "yanked", "license", "downloads", "rust_version")


class InputError(Exception):
    """A condition that exits 2."""


class FetchError(Exception):
    """A request that failed, was refused or returned too much."""


class PackageError(Exception):
    """A package that cannot be prefetched; `status` is refused or failed."""

    def __init__(self, status: str, reason: str):
        super().__init__(reason)
        self.status = status


# --- the network ------------------------------------------------------------


def check_url(url: str) -> str:
    """The host of url when it is https on an allowed host with no user info or port; FetchError otherwise."""
    try:
        parts = urllib.parse.urlsplit(url)
    except ValueError as exc:
        raise FetchError(f"unparsable URL {url!r}") from exc
    if parts.scheme != "https":
        raise FetchError(f"not https: {url!r}")
    if parts.netloc not in HOSTS:
        raise FetchError(f"host not allowed: {parts.netloc!r}")
    return parts.netloc


class Refuse(urllib.request.HTTPRedirectHandler):
    """Every redirect is an error."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def opener() -> urllib.request.OpenerDirector:
    return urllib.request.build_opener(urllib.request.ProxyHandler({}), Refuse())


def make_request(url: str) -> urllib.request.Request:
    return urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json, */*"})


def fetch(url: str, cap: int) -> bytes:
    """The body of url, at most cap bytes; the only network path. Every error is a FetchError."""
    host = check_url(url)
    if host in CRATES_HOSTS:
        time.sleep(RATE_DELAY)
    try:
        with opener().open(make_request(url), timeout=TIMEOUT) as response:
            chunks, size = [], 0
            while size <= cap:
                chunk = response.read(min(cap + 1 - size, MIB))
                if not chunk:
                    break
                chunks.append(chunk)
                size += len(chunk)
    except urllib.error.HTTPError as exc:
        raise FetchError(f"{url}: HTTP {exc.code}") from exc
    except Exception as exc:
        raise FetchError(f"{url}: {type(exc).__name__}: {exc}") from exc
    if size > cap:
        raise FetchError(f"{url}: response over {cap} bytes")
    return b"".join(chunks)


def fetch_json(url: str, cap: int):
    data = fetch(url, cap)
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise FetchError(f"{url}: not JSON") from exc


# --- trimming ---------------------------------------------------------------


def scalar(value, limit: int = TEXT_CAP):
    """value when it is a scalar (strings cut to limit), else None."""
    if isinstance(value, str):
        return value[:limit]
    if value is None or isinstance(value, (bool, int, float)):
        return value
    return None


def mapping(value) -> dict:
    return value if isinstance(value, dict) else {}


def newest(items: list, key: str) -> list:
    return sorted(items, key=lambda item: str(item.get(key) or ""), reverse=True)[:HISTORY]


# --- archives ---------------------------------------------------------------


def gunzip(data: bytes) -> bytes:
    stream = zlib.decompressobj(16 + zlib.MAX_WBITS)
    out, pending = bytearray(), data
    try:
        while pending:
            chunk = stream.decompress(pending, TOTAL_CAP + 1 - len(out))
            out += chunk
            if len(out) > TOTAL_CAP:
                raise PackageError("failed", f"archive over {TOTAL_CAP} bytes decompressed")
            if not chunk and stream.unconsumed_tail == pending:
                break
            pending = stream.unconsumed_tail
    except zlib.error as exc:
        raise PackageError("failed", f"archive is not gzip: {exc}") from exc
    if not stream.eof:
        raise PackageError("failed", "archive gzip stream is truncated")
    return bytes(out)


def unsafe(member: tarfile.TarInfo) -> str:
    """Why member is unsafe, or ""."""
    name = member.name
    if name.startswith("/") or re.match(r"[A-Za-z]:", name):
        return "an absolute path"
    if "\\" in name:
        return "a backslash"
    if any(part in ("", ".", "..") for part in name.split("/")):
        return "a '..', '.' or empty component"
    if member.issym() or member.islnk():
        return "a link"
    if not (member.isfile() or member.isdir()):
        return "neither a file nor a directory"
    if member.size > MEMBER_CAP:
        return f"over {MEMBER_CAP} bytes"
    return ""


def read_archive(data: bytes, top: str = None) -> tuple:
    """(files, read) of a .tar.gz: files maps paths under the single top directory to members; read(path) the bytes.

    top is the required top directory, or None for whatever single one the archive has.
    """
    raw = gunzip(data)
    try:
        tar = tarfile.open(fileobj=io.BytesIO(raw), mode="r:")
        files, count = {}, 0
        while True:
            member = tar.next()
            if member is None:
                break
            count += 1
            if count > MEMBER_COUNT:
                raise PackageError("failed", f"archive has over {MEMBER_COUNT} members")
            why = unsafe(member)
            if why:
                raise PackageError("failed", f"unsafe archive member {member.name!r}: {why}")
            first, _, rest = member.name.partition("/")
            if top is None:
                top = first
            if first != top or (not rest and not member.isdir()):
                raise PackageError("failed", f"archive member {member.name!r} is outside {top + '/'!r}")
            if member.isfile():
                if rest in files:
                    raise PackageError("failed", f"archive member {member.name!r} appears twice")
                files[rest] = member
        if raw[tar.offset :].strip(b"\0"):
            raise PackageError("failed", "archive has data after its end")
    except (tarfile.TarError, EOFError, ValueError) as exc:
        raise PackageError("failed", f"archive cannot be read: {exc}") from exc

    def read(path: str) -> bytes:
        handle = tar.extractfile(files[path])
        return handle.read() if handle is not None else b""

    return files, read


def run_yq(data: bytes):
    """data, a TOML manifest, as JSON through yq on stdin; None when yq cannot read it."""
    try:
        proc = subprocess.run(["yq", "-p=toml", "-o=json", "."], input=data, capture_output=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    try:
        return json.loads(proc.stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        return None


def tidy(path) -> str:
    """A manifest path as an archive path: `./` prefixes dropped; "" when it is not a plain relative path."""
    if not isinstance(path, str):
        return ""
    while path.startswith("./"):
        path = path[2:]
    if not path or path.startswith("/") or "\\" in path or any(p in ("", ".", "..") for p in path.split("/")):
        return ""
    return path


def crate_selection(files: dict, read) -> tuple:
    """(paths to copy, build_time) for a crate."""
    if "Cargo.toml" not in files:
        raise PackageError("failed", "the archive has no Cargo.toml")
    selected, found, undetermined = ["Cargo.toml"], [], False
    manifest = run_yq(read("Cargo.toml")) if files["Cargo.toml"].size <= COPY_CAP else None
    if not isinstance(manifest, dict):
        manifest, undetermined = {}, True
    package, lib = mapping(manifest.get("package")), mapping(manifest.get("lib"))
    build = package.get("build", True)
    if build is False:
        script = ""
    elif build is True:
        script = "build.rs" if "build.rs" in files else ""
    else:
        script = tidy(build)
        if script not in files:
            script, undetermined = "", True
    if script:
        found.append("build.rs" if script == "build.rs" else f"build={script}")
        selected.append(script)
        selected += sorted(p for p in files if p.startswith("build/") and p.endswith(".rs"))
    if lib.get("proc-macro", lib.get("proc_macro")) is True:
        found.append("proc-macro")
        root = tidy(lib.get("path", "src/lib.rs"))
        if root in files:
            selected.append(root)
        else:
            undetermined = True
    return selected, "undetermined" if undetermined else (found or "none")


def npm_selection(files: dict, read) -> tuple:
    """(paths to copy, build_time) for an npm package."""
    if "package.json" not in files:
        raise PackageError("failed", "the archive has no package.json")
    selected, found, undetermined = ["package.json"], [], False
    try:
        manifest = json.loads(read("package.json").decode("utf-8")) if files["package.json"].size <= COPY_CAP else None
    except (UnicodeDecodeError, ValueError):
        manifest = None
    if not isinstance(manifest, dict):
        manifest, undetermined = {}, True
    scripts = manifest.get("scripts", {})
    if not isinstance(scripts, dict):
        scripts, undetermined = {}, True
    for hook in INSTALL_SCRIPTS:
        script = scripts.get(hook)
        if script is None:
            continue
        found.append(hook)
        named = [tidy(token) for token in re.split(r"[\s;&|()<>'\"`=]+", script if isinstance(script, str) else "")]
        named = [path for path in named if path in files]
        if not named:
            undetermined = True
        selected += [path for path in named if path not in selected]
    if "binding.gyp" in files:
        found.append("binding.gyp")
        selected.append("binding.gyp")
    return selected, "undetermined" if undetermined else (found or "none")


# --- the registries ---------------------------------------------------------


def crate(name: str, version: str, checksum: str) -> tuple:
    """(trimmed metadata, archive) of a crate."""
    quoted = urllib.parse.quote(name, safe="")
    api = f"https://crates.io/api/v1/crates/{quoted}"
    meta, owners = fetch_json(api, CRATES_META_CAP), fetch_json(f"{api}/owners", CRATES_META_CAP)
    info, versions = mapping(mapping(meta).get("crate")), mapping(meta).get("versions")
    if not isinstance(versions, list):
        raise PackageError("failed", "the crates.io response has no versions")
    versions = [v for v in versions if isinstance(v, dict)]
    match = [v for v in versions if v.get("num") == version]
    if not match:
        raise PackageError("failed", f"version {version} is not on crates.io")
    release = match[0]
    users = mapping(owners).get("users")
    trimmed = {
        "crate": {k: scalar(info.get(k)) for k in CRATE_FIELDS},
        "version": dict(
            {k: scalar(release.get(k)) for k in VERSION_FIELDS},
            published_by=scalar(mapping(release.get("published_by")).get("login")),
        ),
        "owners": [
            {"login": scalar(u.get("login")), "kind": scalar(u.get("kind"))}
            for u in (users if isinstance(users, list) else [])
            if isinstance(u, dict)
        ],
        "versions_count": len(versions),
        "history": [
            {"num": scalar(v.get("num")), "created_at": scalar(v.get("created_at")), "yanked": scalar(v.get("yanked"))}
            for v in newest(versions, "created_at")
        ],
    }
    archive = fetch(
        f"https://static.crates.io/crates/{quoted}/{quoted}-{urllib.parse.quote(version, safe='')}.crate", ARCHIVE_CAP
    )
    if hashlib.sha256(archive).hexdigest() != checksum.lower():
        raise PackageError("failed", "the archive's SHA-256 differs from the Cargo.lock checksum")
    return trimmed, archive


def sha512_digests(integrity: str) -> list:
    digests = []
    for item in integrity.split():
        if item.startswith("sha512-"):
            try:
                digests.append(base64.b64decode(item[len("sha512-") :], validate=True))
            except ValueError:
                continue
    return digests


def npm(name: str, version: str, integrity: str) -> tuple:
    """(trimmed metadata, archive) of an npm package."""
    digests = sha512_digests(integrity)
    if not digests:
        raise PackageError("failed", "the lockfile integrity has no sha512 hash")
    packument = mapping(fetch_json(NPM_TARBALLS + name.replace("/", "%2f"), NPM_META_CAP))
    versions, times = mapping(packument.get("versions")), mapping(packument.get("time"))
    release = versions.get(version)
    if not isinstance(release, dict):
        raise PackageError("failed", f"version {version} is not on registry.npmjs.org")
    tarball = mapping(release.get("dist")).get("tarball")
    if not isinstance(tarball, str) or not tarball.startswith(NPM_TARBALLS):
        raise PackageError("refused", f"dist.tarball is not on {NPM_TARBALLS}: {scalar(tarball)!r}")
    try:
        check_url(tarball)
    except FetchError as exc:
        raise PackageError("refused", f"dist.tarball: {exc}") from exc
    repository, licence = release.get("repository"), release.get("license")
    scripts = release.get("scripts")
    history = [{"version": v, "time": scalar(times.get(v))} for v in versions]
    for item in history:
        item["deprecated"] = bool(mapping(versions[item["version"]]).get("deprecated"))
    trimmed = {
        "created": scalar(times.get("created")),
        "modified": scalar(times.get("modified")),
        "versions_count": len(versions),
        "history": newest(history, "time"),
        "maintainers": [scalar(m.get("name")) for m in (packument.get("maintainers") or []) if isinstance(m, dict)],
        "publisher": scalar(mapping(release.get("_npmUser")).get("name")),
        "license": scalar(mapping(licence).get("type") if isinstance(licence, dict) else licence),
        "repository": scalar(mapping(repository).get("url") if isinstance(repository, dict) else repository),
        "deprecated": scalar(release.get("deprecated")),
        "scripts": {str(k)[:100]: scalar(v, SCRIPT_CAP) for k, v in mapping(scripts).items()},
        "hasInstallScript": scalar(release.get("hasInstallScript")),
        "downloads": "unavailable",
    }
    archive = fetch(tarball, ARCHIVE_CAP)
    if hashlib.sha512(archive).digest() not in digests:
        raise PackageError("failed", "the archive's SHA-512 differs from the lockfile integrity")
    return trimmed, archive


# --- per package ------------------------------------------------------------


def refusal(entry: dict) -> str:
    """Why entry is refused with no request, or ""."""
    name, version = entry["name"], entry["version"]
    if not entry["registry"]:
        return f"not from the public registry: {entry['source'][:TEXT_CAP]}"
    if entry["ecosystem"] == "cargo":
        if not CRATE_NAME.fullmatch(name):
            return "the name breaks the crates.io name rule"
        if not entry["checksum"]:
            return "Cargo.lock has no checksum for it"
    else:
        if len(name) > 214 or not NPM_NAME.fullmatch(name):
            return "the name breaks the npm name rule"
        if not entry["integrity"]:
            return "pnpm-lock.yaml has no integrity for it"
    if not VERSION.fullmatch(version):
        return "the version breaks the version rule"
    return ""


def prefetch(entry: dict) -> dict:
    """The metadata.json content and the copied files ([(archive path, bytes)]) of a package; PackageError."""
    name, version = entry["name"], entry["version"]
    try:
        if entry["ecosystem"] == "cargo":
            trimmed, archive = crate(name, version, entry["checksum"])
            files, read = read_archive(archive, f"{name}-{version}")
            selected, build_time = crate_selection(files, read)
        else:
            trimmed, archive = npm(name, version, entry["integrity"])
            files, read = read_archive(archive)
            selected, build_time = npm_selection(files, read)
    except FetchError as exc:
        raise PackageError("failed", str(exc)) from exc
    copied = []
    for path in selected:
        if files[path].size > COPY_CAP:
            raise PackageError("failed", f"{path} is over {COPY_CAP} bytes")
        copied.append((path, read(path)))
    return {"registry": trimmed, "build_time": build_time}, copied


def write_json(path: Path, value) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(0o644)


def write_package(target: Path, metadata: dict, copied: list) -> None:
    """Write the package directory under a temporary sibling name, then rename it into place."""
    temporary = target.parent / f".{target.name}.partial"
    remove(temporary)
    try:
        (temporary / "files").mkdir(parents=True)
        listed = []
        for number, (path, data) in enumerate(copied, 1):
            file = temporary / "files" / f"{number}.txt"
            file.write_bytes(data)
            file.chmod(0o644)
            listed.append(
                {
                    "archive_path": path,
                    "file": f"files/{number}.txt",
                    "bytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
            )
        write_json(temporary / "metadata.json", dict(metadata, files=listed))
        temporary.rename(target)
    except OSError:
        remove(temporary)
        raise


def remove(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.exists():
        shutil.rmtree(path)


# --- packages.json ----------------------------------------------------------


def load(path: Path) -> list:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        raise InputError(f"{path} cannot be read: {exc}") from exc
    packages = data.get("packages") if isinstance(data, dict) else None
    if not isinstance(packages, list):
        raise InputError(f"{path} has no packages list")
    for number, entry in enumerate(packages, 1):
        if not (
            isinstance(entry, dict)
            and entry.get("ecosystem") in ("cargo", "npm")
            and all(isinstance(entry.get(k), str) for k in ("lockfile", "name", "version", "source"))
            and isinstance(entry.get("registry"), bool)
            and all(k in entry and (entry[k] is None or isinstance(entry[k], str)) for k in ("checksum", "integrity"))
        ):
            raise InputError(f"{path}: package {number} is not the shape new-packages.py writes")
    return packages


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise InputError(message)


def main(argv: list) -> int:
    parser = Parser(prog="prefetch.py", add_help=False)
    parser.add_argument("--dir", required=True)
    try:
        args = parser.parse_args(argv)
        out = Path(args.dir)
        try:
            for stale in ("prefetch.json", "cargo", "npm"):
                remove(out / stale)
        except OSError as exc:
            raise InputError(f"cannot remove the stale output: {exc}") from exc
        if not (out / "packages.json").is_file():
            print("no packages.json")
            return 0
        packages = load(out / "packages.json")
        records, done = [], {}
        for entry in packages:
            record = {k: entry[k] for k in ("ecosystem", "lockfile", "name", "version")}
            record.update(status="fetched", reason="", dir=None)
            key = (entry["ecosystem"], entry["name"], entry["version"])
            digest = entry["checksum"] if entry["ecosystem"] == "cargo" else entry["integrity"]
            why = refusal(entry)
            if why:
                record.update(status="refused", reason=why)
            elif key in done and done[key][0] != digest:
                record.update(status="failed", reason="another lockfile entry for this version has a different hash")
            elif key in done:
                record.update(done[key][1])
            else:
                try:
                    metadata, copied = prefetch(entry)
                    relative = f"{entry['ecosystem']}/{entry['name']}@{entry['version']}"
                    target = out / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    write_package(target, metadata, copied)
                    record["dir"] = relative
                except PackageError as exc:
                    record.update(status=exc.status, reason=str(exc))
                except OSError as exc:
                    raise InputError(f"cannot write {entry['name']}@{entry['version']}: {exc}") from exc
                done[key] = (digest, {k: record[k] for k in ("status", "reason", "dir")})
            records.append(record)
        try:
            write_json(out / "prefetch.json", {"packages": records})
        except OSError as exc:
            raise InputError(f"cannot write {out / 'prefetch.json'}: {exc}") from exc
    except InputError as exc:
        print(f"prefetch.py: {exc}", file=sys.stderr)
        return 2
    counts = {s: sum(r["status"] == s for r in records) for s in ("fetched", "refused", "failed")}
    print(f"{counts['fetched']} fetched, {counts['refused']} refused, {counts['failed']} failed: {out}/prefetch.json")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
