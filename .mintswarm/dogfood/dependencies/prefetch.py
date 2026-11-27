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
memory: data after the gzip stream or after the tar end, an absolute
member, a `..`, `.` or empty component, a backslash, a link, anything but a
regular file or a directory, two paths equal when case and Unicode
normalisation are ignored, too many members or a member outside the single
top directory fails the archive as a whole, and so does a selection-relevant
name that matches a member only when case is ignored. Nothing is extracted to disk by tarfile and nothing from the package
is executed; the crate's Cargo.toml is parsed by `yq -p=toml` on stdin, the
only subprocess.

For a fetched package it writes `<dir>/<ecosystem>/<name>@<version>/`
holding `metadata.json` (`registry`: the trimmed registry metadata;
`found`: the build or install code found; `build_time`: `none`, that list,
`partial` when the copied code references files that were not copied, or
`undetermined`; `files`: what was copied) and `files/1.txt`, `files/2.txt`,
… (the manifest and the build or install code, mode 0644). `<dir>/prefetch.json`
lists every package with its `status`, `reason` and `dir`.

Beside the newest HISTORY releases, the trimmed metadata names the release
published right before the locked version, so that the review-time screen
can tell a change of publisher and the version's age: a crate's `previous`
(`num`, `created_at` and the `published_by` login), and an npm package's
`time` (the locked version's publish time) and `previous` (`version`,
`time` and the `_npmUser` name). Each is null when there is no earlier
release or, for npm, when only the version document was read. When the
locked version's own publish time is missing, `previous` is left out, so
that an unknown earlier release is never read as a first release.

Standard library only, apart from running yq. Exit 0 whatever happens to
single packages; 2 on bad arguments, a malformed packages.json or an output
that cannot be written.
"""

import argparse
import base64
import hashlib
import io
import json
import posixpath
import re
import shutil
import subprocess
import sys
import tarfile
import time
import unicodedata
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
# An archive's peak memory is about ARCHIVE_CAP + TOTAL_CAP: the compressed body and one decompressed buffer.
ARCHIVE_CAP = 64 * MIB
TOTAL_CAP = 128 * MIB
MEMBER_CAP = 32 * MIB
MEMBER_COUNT = 20000
COPY_CAP = 1 * MIB
COPY_TOTAL = 16 * MIB
HISTORY = 20
TEXT_CAP = 500
SCRIPT_CAP = 1000
CRATE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}")
NPM_NAME = re.compile(r"(@[a-z0-9~-][a-z0-9._~-]*/)?[a-z0-9~-][a-z0-9._~-]*")
VERSION = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+-]{0,63}")
INSTALL_SCRIPTS = ("preinstall", "install", "postinstall")
CRATE_FIELDS = ("created_at", "updated_at", "downloads", "recent_downloads", "repository", "homepage")
VERSION_FIELDS = ("created_at", "yanked", "license", "downloads", "rust_version")
MEMBER_TYPES = (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE)


class InputError(Exception):
    """A condition that exits 2."""


class FetchError(Exception):
    """A request that failed, was refused or returned too much."""


class OverCap(FetchError):
    """A response over its cap."""


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


def fetch(url: str, cap: int) -> bytearray:
    """The body of url as one bytearray, at most cap bytes; the only network path. Every error is a FetchError."""
    host = check_url(url)
    if host in CRATES_HOSTS:
        time.sleep(RATE_DELAY)
    try:
        with opener().open(make_request(url), timeout=TIMEOUT) as response:
            body = bytearray()
            while len(body) <= cap:
                chunk = response.read(min(cap + 1 - len(body), MIB))
                if not chunk:
                    break
                body += chunk
    except urllib.error.HTTPError as exc:
        raise FetchError(f"{url}: HTTP {exc.code}") from exc
    except Exception as exc:
        raise FetchError(f"{url}: {type(exc).__name__}: {exc}") from exc
    if len(body) > cap:
        raise OverCap(f"{url}: response over {cap} bytes")
    return body


def fetch_json(url: str, cap: int):
    data = fetch(url, cap)
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
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


def gunzip(data) -> bytearray:
    """data, one gzip member and nothing after it, decompressed to at most TOTAL_CAP bytes.

    The input is read in MIB slices and each call yields at most MIB, so no full copy of either is made.
    """
    stream = zlib.decompressobj(16 + zlib.MAX_WBITS)
    view, out, offset, pending = memoryview(data), bytearray(), 0, b""
    try:
        while not stream.eof:
            if not pending:
                if offset >= len(view):
                    break
                pending = view[offset : offset + MIB]
                offset += len(pending)
            chunk = stream.decompress(pending, min(MIB, TOTAL_CAP + 1 - len(out)))
            out += chunk
            if len(out) > TOTAL_CAP:
                raise PackageError("failed", f"archive over {TOTAL_CAP} bytes decompressed")
            if not chunk and len(stream.unconsumed_tail) == len(pending):
                break
            pending = stream.unconsumed_tail
    except zlib.error as exc:
        raise PackageError("failed", f"archive is not gzip: {exc}") from exc
    if not stream.eof:
        raise PackageError("failed", "archive gzip stream is truncated")
    if stream.unused_data or offset < len(view):
        raise PackageError("failed", "archive has data after its gzip stream")
    return out


def fold(path: str) -> str:
    """path as a case-insensitive, normalisation-insensitive file system sees it."""
    return unicodedata.normalize("NFC", path).casefold()


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
    if member.type not in MEMBER_TYPES:
        return "not a regular file or a directory"
    return ""


class ViewReader(io.RawIOBase):
    """A seekable, read-only file over a bytes-like value, without the copy io.BytesIO makes of a bytearray."""

    def __init__(self, data):
        super().__init__()
        self.view, self.position = memoryview(data), 0

    def readable(self) -> bool:
        return True

    def seekable(self) -> bool:
        return True

    def tell(self) -> int:
        return self.position

    def seek(self, offset: int, whence: int = io.SEEK_SET) -> int:
        base = {io.SEEK_SET: 0, io.SEEK_CUR: self.position, io.SEEK_END: len(self.view)}[whence]
        if base + offset < 0:
            raise ValueError("negative seek position")
        self.position = base + offset
        return self.position

    def readinto(self, buffer) -> int:
        piece = self.view[self.position : self.position + len(buffer)]
        memoryview(buffer).cast("B")[: len(piece)] = piece
        self.position += len(piece)
        return len(piece)


class Archive:
    """The files of a .tar.gz under its single top directory, read in memory; PackageError when unsafe.

    top is the required top directory, or None for whatever single one the archive has.
    """

    def __init__(self, data: bytes, top: str = None):
        raw = gunzip(data)
        self.files, self.folded = {}, {}
        try:
            self.tar = tarfile.open(fileobj=ViewReader(raw), mode="r:")
            count = 0
            while True:
                member = self.tar.next()
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
                if not rest:
                    continue
                if fold(rest) in self.folded and (self.folded[fold(rest)] != rest or not member.isdir()):
                    raise PackageError(
                        "failed", f"archive member {member.name!r} collides with {self.folded[fold(rest)]!r}"
                    )
                self.folded[fold(rest)] = rest
                if member.isfile():
                    self.files[rest] = member
            if raw.count(b"\0", self.tar.offset) != len(raw) - self.tar.offset:
                raise PackageError("failed", "archive has data after its end")
        except (tarfile.TarError, EOFError, ValueError) as exc:
            raise PackageError("failed", f"archive cannot be read: {exc}") from exc

    def has(self, path: str) -> bool:
        """Whether path is a file; PackageError when it matches a member only under case or Unicode folding."""
        if path in self.files:
            return True
        other = self.folded.get(fold(path)) if path else None
        if other is not None and other != path:
            raise PackageError("failed", f"archive member {other!r} matches {path!r} only when case is ignored")
        return False

    def rust_under(self, directory: str) -> list:
        """The .rs files under directory ("" for all); PackageError for one whose directory matches only folded."""
        prefix = directory + "/" if directory else ""
        found = []
        for path in self.files:
            if not path.endswith(".rs"):
                continue
            if path.startswith(prefix):
                found.append(path)
            elif fold(path).startswith(fold(prefix)):
                raise PackageError("failed", f"archive member {path!r} matches {prefix!r} only when case is ignored")
        return sorted(found)

    def read(self, path: str) -> bytes:
        member = self.files[path]
        if member.size > MEMBER_CAP:
            raise PackageError("failed", f"{path} is over {MEMBER_CAP} bytes")
        handle = self.tar.extractfile(member)
        return handle.read() if handle is not None else b""


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
    except (UnicodeDecodeError, ValueError, RecursionError):
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


def is_proc_macro(lib) -> tuple:
    """(proc-macro, undetermined) for a [lib] table: `proc-macro`/`proc_macro` true or `crate-type` with proc-macro."""
    if lib is None:
        return False, False
    if not isinstance(lib, dict):
        return False, True
    flag, undetermined = False, False
    for key in ("proc-macro", "proc_macro"):
        value = lib.get(key)
        if value is True:
            flag = True
        elif value is not None and value is not False:
            undetermined = True
    for key in ("crate-type", "crate_type"):
        value = lib.get(key)
        if value is None:
            continue
        if isinstance(value, list) and all(isinstance(kind, str) for kind in value):
            flag = flag or "proc-macro" in value
        else:
            undetermined = True
    return flag, undetermined


def crate_selection(archive: Archive) -> tuple:
    """(paths to copy, found, undetermined, crate root files) for a crate."""
    if not archive.has("Cargo.toml"):
        raise PackageError("failed", "the archive has no Cargo.toml")
    selected, found, undetermined = ["Cargo.toml"], [], False
    manifest = run_yq(archive.read("Cargo.toml")) if archive.files["Cargo.toml"].size <= COPY_CAP else None
    if not isinstance(manifest, dict):
        manifest, undetermined = {}, True
    package = mapping(manifest.get("package"))
    build = package.get("build", True)
    if build is False:
        script = ""
    elif build is True:
        script = "build.rs" if archive.has("build.rs") else ""
    else:
        script = tidy(build)
        if not script or not archive.has(script):
            script, undetermined = "", True
    roots = set()
    if script:
        roots.add(script)
        found.append("build.rs" if script == "build.rs" else f"build={script}")
        selected.append(script)
        selected += archive.rust_under("build")
    proc_macro, unknown = is_proc_macro(manifest.get("lib"))
    undetermined = undetermined or unknown
    if proc_macro:
        found.append("proc-macro")
        root = tidy(mapping(manifest.get("lib")).get("path", "src/lib.rs"))
        if root and archive.has(root):
            roots.add(root)
            selected += [root] + archive.rust_under(root.rpartition("/")[0])
        else:
            undetermined = True
    return list(dict.fromkeys(selected)), found, undetermined, roots


def npm_selection(archive: Archive) -> tuple:
    """(paths to copy, found, undetermined, crate root files) for an npm package; the roots are always empty."""
    if not archive.has("package.json"):
        raise PackageError("failed", "the archive has no package.json")
    selected, found, undetermined = ["package.json"], [], False
    try:
        data = archive.read("package.json") if archive.files["package.json"].size <= COPY_CAP else b""
        manifest = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError):
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
        tokens = re.split(r"[\s;&|()<>'\"`=]+", script if isinstance(script, str) else "")
        named = [path for path in (tidy(token) for token in tokens) if path and archive.has(path)]
        if not named:
            undetermined = True
        selected += named
    if archive.has("binding.gyp"):
        found.append("binding.gyp")
        selected.append("binding.gyp")
    return list(dict.fromkeys(selected)), found, undetermined, set()


RUST_TOKEN = re.compile(r"//|/\*|\bb?r#*\"|\"|'")
RUST_CHAR = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|[^\n])|[^\\'\n])'")
RUST_STRING = re.compile(r'"(?:\\.|[^"\\])*"', re.S)
RUST_MOD = re.compile(r"mod\s+(?:r#)?([A-Za-z_][A-Za-z0-9_]*)\s*([;{])")
RUST_ATTRIBUTE = re.compile(r"#\s*!?\s*\[")
RUST_MACRO_RULES = re.compile(r"\bmacro_rules\s*!")
RUST_INCLUDE = re.compile(r"\binclude(?:_str|_bytes)?\s*!")
JS_STRING = re.compile(r"""(['"`])((?:\\.|(?!\1).){1,512}?)\1""")
JS_LOADER = re.compile(r"\b(?:require|import)\b")
JS_LOAD = re.compile(r"""(?:require|import)\(\s*(['"])[^'"\\\n]*\1\s*\)""")
JS_INDIRECT = re.compile(r"\b(?:eval|Function|createRequire|_load|child_process|vm)\b")
JS_EXTENSIONS = ("", ".js", ".cjs", ".mjs", ".json", "/index.js", "/index.cjs", "/index.json")
BRACKETS = {"(": ")", "[": "]", "{": "}"}


def strip_rust(text: str):
    """text with comments (line and nested block) replaced by a space and string and char literals by empty
    ones, so nothing hides in either; None when a comment or literal does not end."""
    out, at = [], 0
    while True:
        match = RUST_TOKEN.search(text, at)
        if match is None:
            out.append(text[at:])
            return "".join(out)
        start, token = match.start(), match.group()
        out.append(text[at:start])
        if token == "//":
            end = text.find("\n", start)
            at = len(text) if end < 0 else end
            out.append(" ")
        elif token == "/*":
            depth, at = 1, start + 2
            while depth:
                opening, closing = text.find("/*", at), text.find("*/", at)
                if closing < 0:
                    return None
                if 0 <= opening < closing:
                    depth, at = depth + 1, opening + 2
                else:
                    depth, at = depth - 1, closing + 2
            out.append(" ")
        elif token.endswith('"') and token != '"':
            closing = '"' + "#" * token.count("#")
            end = text.find(closing, match.end())
            if end < 0:
                return None
            at = end + len(closing)
            out.append('""')
        elif token == '"':
            literal = RUST_STRING.match(text, start)
            if literal is None:
                return None
            at = literal.end()
            out.append('""')
        else:
            literal = RUST_CHAR.match(text, start)
            at = literal.end() if literal else start + 1
            out.append("' '" if literal else "'")


def bracketed(text: str, start: int):
    """The text inside the bracket group opening at or after start, or None when there is none or it never closes."""
    opening = re.compile(r"[(\[{]").search(text, start)
    if opening is None:
        return None
    stack, at = [BRACKETS[opening.group()]], opening.end()
    for token in re.compile(r"[(\[{)\]}]").finditer(text, at):
        if token.group() in BRACKETS:
            stack.append(BRACKETS[token.group()])
        elif token.group() != stack.pop():
            return None
        if not stack:
            return text[at : token.start()]
    return None


def rust_partial(path: str, text: str, copied: set, roots: set) -> bool:
    """Whether Rust file path may run code that was not copied. Fail safe: after comments and literals are
    stripped, an include!, a `path` key in any attribute, a macro_rules! body with `mod`, a `mod` not of the form
    `mod <ident>;` or `mod <ident> {`, unbalanced braces, or a `mod <ident>;` whose file was not copied."""
    code = strip_rust(text)
    if code is None or RUST_INCLUDE.search(code):
        return True
    for attribute in RUST_ATTRIBUTE.finditer(code):
        inside = bracketed(code, attribute.end() - 1)
        if inside is None or re.search(r"\bpath\s*=", inside):
            return True
    for macro in RUST_MACRO_RULES.finditer(code):
        body = bracketed(code, macro.end())
        if body is None or re.search(r"\bmod\b", body):
            return True
    directory, _, file = path.rpartition("/")
    stem = file[: -len(".rs")]
    base = directory if path in roots or stem in ("mod", "lib", "main") else posixpath.join(directory, stem)
    stack, pending = [], None
    for token in re.finditer(r"\{|\}|\bmod\b", code):
        if token.group() == "{":
            stack.append(pending)
            pending = None
        elif token.group() == "}":
            if not stack:
                return True
            stack.pop()
        else:
            declaration = RUST_MOD.match(code, token.start())
            if declaration is None:
                return True
            module, end = declaration.groups()
            if end == "{":
                pending = module
                continue
            where = posixpath.join(base, *[name for name in stack if name])
            candidates = (posixpath.join(where, f"{module}.rs"), posixpath.join(where, module, "mod.rs"))
            if not any(candidate in copied for candidate in candidates):
                return True
    return bool(stack)


def js_partial(path: str, text: str, copied: set, archive: Archive) -> bool:
    """Whether install script path may run a file that was not copied. Fail safe: a `require` or `import` not
    immediately followed by `(` and one string literal, an indirect loader (eval, Function, createRequire, vm,
    child_process, _load), or a string literal that resolves, from its directory or the package root, to an
    archive file that was not copied."""
    if JS_INDIRECT.search(text):
        return True
    if any(not JS_LOAD.match(text, loader.start()) for loader in JS_LOADER.finditer(text)):
        return True
    directory = path.rpartition("/")[0]
    for _, literal in JS_STRING.findall(text):
        for start in {directory, ""}:
            joined = posixpath.normpath(posixpath.join(start, literal))
            if joined.startswith("../") or joined == "..":
                if literal.startswith(("./", "../")):
                    return True
                continue
            for suffix in JS_EXTENSIONS:
                candidate = tidy(joined + suffix)
                if candidate and candidate not in copied and archive.has(candidate):
                    return True
            if literal.startswith(("./", "../")) and not any(tidy(joined + s) in copied for s in JS_EXTENSIONS):
                return True
    return False


def partial(ecosystem: str, archive: Archive, copied: list, roots: set) -> bool:
    """Whether the copied build or install code references files that were not copied."""
    paths = {path for path, _ in copied}
    for path, data in copied:
        text = data.decode("utf-8", errors="replace")
        if ecosystem == "cargo":
            if path.endswith(".rs") and rust_partial(path, text, paths, roots):
                return True
        elif path == "binding.gyp":
            return True
        elif path != "package.json":
            if not path.endswith((".js", ".cjs", ".mjs")) or js_partial(path, text, paths, archive):
                return True
    return False


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
    locked = str(release.get("created_at") or "")
    earlier = [v for v in versions if locked and str(v.get("created_at") or "") < locked]
    before = max(earlier, key=lambda v: str(v.get("created_at") or ""), default=None)
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
        "previous": (
            {
                "num": scalar(before.get("num")),
                "created_at": scalar(before.get("created_at")),
                "published_by": scalar(mapping(before.get("published_by")).get("login")),
            }
            if before is not None
            else None
        ),
    }
    if not locked:
        del trimmed["previous"]
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
    """(trimmed metadata, archive) of an npm package.

    The packument gives the release history; when it is over NPM_META_CAP, the version document
    `<name>/<version>` alone is used and the history fields are null.
    """
    digests = sha512_digests(integrity)
    if not digests:
        raise PackageError("failed", "the lockfile integrity has no sha512 hash")
    url = NPM_TARBALLS + name.replace("/", "%2f")
    try:
        packument, source = mapping(fetch_json(url, NPM_META_CAP)), "packument"
        versions, times = mapping(packument.get("versions")), mapping(packument.get("time"))
        release = versions.get(version)
    except OverCap:
        packument, source = {}, "version document"
        versions, times = None, {}
        release = fetch_json(f"{url}/{urllib.parse.quote(version, safe='')}", NPM_META_CAP)
        if isinstance(release, dict) and release.get("version") != version:
            release = None
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
    maintainers = packument.get("maintainers") if source == "packument" else release.get("maintainers")
    history, locked, previous = None, None, None
    if versions is not None:
        locked = scalar(times.get(version)) if isinstance(times.get(version), str) else None
        earlier = [v for v in versions if locked and isinstance(times.get(v), str) and times[v] < locked]
        if earlier:
            before = max(earlier, key=lambda v: times[v])
            previous = {
                "version": scalar(before),
                "time": scalar(times[before]),
                "publisher": scalar(mapping(mapping(versions[before]).get("_npmUser")).get("name")),
            }
        history = [{"version": v, "time": scalar(times.get(v))} for v in versions]
        for item in history:
            item["deprecated"] = bool(mapping(versions[item["version"]]).get("deprecated"))
        history = newest(history, "time")
    trimmed = {
        "source": source,
        "created": scalar(times.get("created")),
        "modified": scalar(times.get("modified")),
        "versions_count": len(versions) if versions is not None else None,
        "history": history,
        "time": locked,
        "previous": previous,
        "maintainers": [scalar(m.get("name")) for m in (maintainers or []) if isinstance(m, dict)],
        "publisher": scalar(mapping(release.get("_npmUser")).get("name")),
        "license": scalar(mapping(licence).get("type") if isinstance(licence, dict) else licence),
        "repository": scalar(mapping(repository).get("url") if isinstance(repository, dict) else repository),
        "deprecated": scalar(release.get("deprecated")),
        "scripts": {str(k)[:100]: scalar(v, SCRIPT_CAP) for k, v in mapping(release.get("scripts")).items()},
        "hasInstallScript": scalar(release.get("hasInstallScript")),
        "downloads": "unavailable",
    }
    if versions is not None and locked is None:
        del trimmed["previous"]
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


def prefetch(entry: dict) -> tuple:
    """The metadata.json content and the copied files ([(archive path, bytes)]) of a package.

    Every error, whatever its kind, is a PackageError, so one package never stops the run.
    """
    try:
        return prefetch_package(entry)
    except PackageError:
        raise
    except FetchError as exc:
        raise PackageError("failed", str(exc)) from exc
    except Exception as exc:
        raise PackageError("failed", f"{type(exc).__name__}: {str(exc)[:TEXT_CAP]}") from exc


def prefetch_package(entry: dict) -> tuple:
    name, version, ecosystem = entry["name"], entry["version"], entry["ecosystem"]
    if ecosystem == "cargo":
        trimmed, data = crate(name, version, entry["checksum"])
        archive = Archive(data, f"{name}-{version}")
        selected, found, undetermined, roots = crate_selection(archive)
    else:
        trimmed, data = npm(name, version, entry["integrity"])
        archive = Archive(data)
        selected, found, undetermined, roots = npm_selection(archive)
    copied, total = [], 0
    for path in selected:
        if archive.files[path].size > COPY_CAP:
            raise PackageError("failed", f"{path} is over {COPY_CAP} bytes")
        total += archive.files[path].size
        if total > COPY_TOTAL:
            raise PackageError("failed", f"the selected files are over {COPY_TOTAL} bytes together")
        copied.append((path, archive.read(path)))
    if undetermined:
        build_time = "undetermined"
    elif found and partial(ecosystem, archive, copied, roots):
        build_time = "partial"
    else:
        build_time = found or "none"
    return {"registry": trimmed, "build_time": build_time, "found": found}, copied


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
