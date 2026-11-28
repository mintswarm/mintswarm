#!/usr/bin/env python3
"""Vendors the OTLP protos from the pinned opentelemetry-proto release.

Reads the tag and the archive's SHA-256 from VENDOR beside this script, fetches
the tag archive from github.com anonymously, checks its SHA-256 and writes the
protos of the OTLP packages under proto/ and the release's LICENSE beside this
script, byte for byte.

With no hash recorded the script refuses to fetch, unless --record-first-hash
is given; that flag fetches the pinned tag once and writes the archive's hash
into VENDOR, and is refused when a hash is already recorded.

Exit status: 0 on success, 1 on any refusal or failure.
"""

import argparse
import hashlib
import io
import re
import sys
import tarfile
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
VENDOR = HERE / "VENDOR"
LICENSE = HERE / "LICENSE"
PROTO = HERE / "proto"

URL = "https://github.com/open-telemetry/opentelemetry-proto/archive/refs/tags/{tag}.tar.gz"
HOSTS = ("github.com", "codeload.github.com")
TIMEOUT = 60

# The vendored packages' directories, relative to opentelemetry/proto/ in the archive.
PACKAGES = (
    "common/v1",
    "resource/v1",
    "trace/v1",
    "logs/v1",
    "metrics/v1",
    "collector/trace/v1",
    "collector/logs/v1",
    "collector/metrics/v1",
)


class Refused(Exception):
    """A refusal or failure, reported as one line on stderr."""


def read_vendor():
    """Returns the tag and the recorded hash (empty when none) from VENDOR."""
    tag = None
    digest = None
    for line in VENDOR.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        name, _, value = line.partition(" ")
        if name == "tag":
            tag = value.strip()
        elif name == "sha256":
            digest = value.strip()
        else:
            raise Refused(f"VENDOR: unknown line {line!r}")
    if not tag or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise Refused("VENDOR: no tag of the form vX.Y.Z")
    if digest is None:
        raise Refused("VENDOR: no sha256 line")
    if digest and not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise Refused("VENDOR: the sha256 is not 64 lowercase hex digits")
    return tag, digest


def write_hash(digest):
    """Writes digest into VENDOR's sha256 line, leaving the other lines as they are."""
    lines = VENDOR.read_text(encoding="utf-8").splitlines(keepends=True)
    out = []
    for line in lines:
        if line.startswith("sha256"):
            line = f"sha256 {digest}\n"
        out.append(line)
    VENDOR.write_text("".join(out), encoding="utf-8")


def allowed(url):
    """Whether url is https on one of GitHub's download hosts."""
    parts = urllib.parse.urlsplit(url)
    return parts.scheme == "https" and parts.hostname in HOSTS


class Redirects(urllib.request.HTTPRedirectHandler):
    """Follows only redirects to https on GitHub's download hosts."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not allowed(newurl):
            raise Refused(f"refusing a redirect to {newurl}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def fetch(tag):
    """Returns the bytes of the tag archive, fetched with no credential."""
    url = URL.format(tag=tag)
    opener = urllib.request.build_opener(Redirects)
    request = urllib.request.Request(url, headers={"User-Agent": "vendor.py"})
    try:
        with opener.open(request, timeout=TIMEOUT) as response:
            if not allowed(response.geturl()):
                raise Refused(f"refusing the final URL {response.geturl()}")
            return response.read()
    except OSError as error:
        raise Refused(f"fetching {url} failed: {error}") from error


def members(data):
    """Returns {relative path: bytes} for LICENSE and the vendored protos in the archive."""
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            parts = member.name.split("/")
            if ".." in parts or member.name.startswith("/"):
                raise Refused(f"refusing the archive member {member.name}")
            if len(parts) < 2:
                continue
            rest = "/".join(parts[1:])
            if rest == "LICENSE":
                target = "LICENSE"
            else:
                directory, _, name = rest.rpartition("/")
                prefix = "opentelemetry/proto/"
                if not directory.startswith(prefix) or not name.endswith(".proto"):
                    continue
                if directory[len(prefix) :] not in PACKAGES:
                    continue
                target = "proto/" + rest[len(prefix) :]
            if not member.isreg():
                raise Refused(f"refusing the non-regular member {member.name}")
            files[target] = archive.extractfile(member).read()
    if "LICENSE" not in files:
        raise Refused("the archive holds no LICENSE")
    for package in PACKAGES:
        if not any(path.startswith(f"proto/{package}/") for path in files):
            raise Refused(f"the archive holds no protos under {package}")
    return files


def write(files):
    """Writes files under HERE and removes any other .proto under proto/."""
    keep = {HERE / path for path in files}
    if PROTO.exists():
        for path in PROTO.rglob("*.proto"):
            if path not in keep:
                path.unlink()
    for path, data in files.items():
        target = HERE / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)


def run(record_first_hash):
    tag, recorded = read_vendor()
    if not recorded and not record_first_hash:
        raise Refused("no hash recorded in VENDOR; only an explicit --record-first-hash run records one")
    if recorded and record_first_hash:
        raise Refused("a hash is already recorded in VENDOR; --record-first-hash is refused")
    data = fetch(tag)
    digest = hashlib.sha256(data).hexdigest()
    if recorded and digest != recorded:
        raise Refused(f"the archive's SHA-256 {digest} differs from the recorded {recorded}")
    files = members(data)
    write(files)
    if record_first_hash:
        write_hash(digest)
    protos = sum(1 for path in files if path.endswith(".proto"))
    print(f"vendored {tag}: {protos} protos and LICENSE")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--record-first-hash",
        action="store_true",
        help="fetch the pinned tag once and record its archive's SHA-256 in VENDOR",
    )
    args = parser.parse_args()
    try:
        run(args.record_first_hash)
    except Refused as error:
        print(f"vendor.py: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
