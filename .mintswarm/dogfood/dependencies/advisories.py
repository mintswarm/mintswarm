"""Advisory sources for the dependency screen: the interface and its OSV implementation.

An advisory source answers, for a batch of packages, which known security
advisories affect each one's exact version. `screen.py` reaches advisories
only through the `AdvisorySource` interface; `OsvSource` is the first
implementation, and another source can be added beside it.

`OsvSource` asks `api.osv.dev`, which covers RustSec and the GitHub
advisories, with the ecosystems `crates.io` and `npm`. It POSTs
`/v1/querybatch` in chunks of 1,000 queries, follows `next_page_token`, and
GETs `/v1/vulns/<id>` for each advisory id. An entry that carries `withdrawn`
is dropped; every other entry is kept, informational RustSec ones included.

This module opens no socket: `OsvSource` takes the transport, a callable
`transport(method, url, cap, body=None) -> bytes` that raises on any error,
from its caller. Standard library only.
"""

import json
import urllib.parse
from typing import NamedTuple, Protocol

OSV = "https://api.osv.dev"
QUERYBATCH = OSV + "/v1/querybatch"
ECOSYSTEMS = frozenset({"crates.io", "npm"})
CHUNK = 1000
MAX_PAGES = 20
BATCH_CAP = 16 * 1024 * 1024
VULN_CAP = 4 * 1024 * 1024
TEXT_CAP = 500


class Advisory(NamedTuple):
    """One advisory that affects a package version."""

    id: str
    summary: str
    source: str
    aliases: tuple

    def record(self) -> dict:
        return {"id": self.id, "summary": self.summary, "source": self.source, "aliases": list(self.aliases)}


class AdvisoryError(Exception):
    """An advisory query that failed; the packages it covers cannot be screened."""


class AdvisorySource(Protocol):
    """Known advisories for packages.

    `query(packages)` takes (ecosystem, name, version) tuples and returns a dict keyed by each of them. A value is
    the list of `Advisory` that affect that version (empty when none do), or an `AdvisoryError` when that package's
    advisories could not be fully read. A failure of the whole query raises `AdvisoryError`.
    """

    name: str

    def query(self, packages: list) -> dict: ...


def text(value) -> str:
    return value[:TEXT_CAP] if isinstance(value, str) else ""


def parse(data: bytes, where: str):
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise AdvisoryError(f"{where}: not JSON") from exc


class OsvSource:
    """Advisories from api.osv.dev through the caller's transport."""

    name = "osv"

    def __init__(self, transport):
        self.transport = transport

    def call(self, method: str, url: str, cap: int, body: bytes = None):
        try:
            data = self.transport(method, url, cap, body)
        except Exception as exc:
            raise AdvisoryError(f"{url}: {exc}") from exc
        return parse(data, url)

    def ids(self, packages: list) -> dict:
        """The advisory ids osv.dev lists for each package, every page followed."""
        found = {key: [] for key in packages}
        pending = [(key, None) for key in packages]
        for _ in range(MAX_PAGES):
            if not pending:
                return found
            following = []
            for start in range(0, len(pending), CHUNK):
                chunk = pending[start : start + CHUNK]
                queries = []
                for (ecosystem, name, version), token in chunk:
                    query = {"package": {"ecosystem": ecosystem, "name": name}, "version": version}
                    if token is not None:
                        query["page_token"] = token
                    queries.append(query)
                body = json.dumps({"queries": queries}, sort_keys=True).encode("utf-8")
                answer = self.call("POST", QUERYBATCH, BATCH_CAP, body)
                results = answer.get("results") if isinstance(answer, dict) else None
                if not isinstance(results, list) or len(results) != len(chunk):
                    raise AdvisoryError("querybatch: the results do not match the queries")
                for (key, _), result in zip(chunk, results):
                    if not isinstance(result, dict):
                        raise AdvisoryError("querybatch: a result is not an object")
                    vulns = result.get("vulns", [])
                    if not isinstance(vulns, list):
                        raise AdvisoryError("querybatch: vulns is not a list")
                    for vuln in vulns:
                        ident = vuln.get("id") if isinstance(vuln, dict) else None
                        if not isinstance(ident, str) or not ident:
                            raise AdvisoryError("querybatch: an entry has no id")
                        if ident not in found[key]:
                            found[key].append(ident)
                    token = result.get("next_page_token")
                    if token:
                        if not isinstance(token, str):
                            raise AdvisoryError("querybatch: next_page_token is not a string")
                        following.append((key, token))
            pending = following
        if pending:
            raise AdvisoryError(f"querybatch: more than {MAX_PAGES} pages")
        return found

    def details(self, ident: str):
        """The Advisory for ident, or None when it is withdrawn."""
        url = f"{OSV}/v1/vulns/{urllib.parse.quote(ident, safe='')}"
        entry = self.call("GET", url, VULN_CAP)
        if not isinstance(entry, dict) or entry.get("id") != ident:
            raise AdvisoryError(f"{url}: not the entry for {ident}")
        if entry.get("withdrawn"):
            return None
        aliases = entry.get("aliases")
        aliases = tuple(sorted(text(a) for a in aliases if isinstance(a, str))) if isinstance(aliases, list) else ()
        summary = text(entry.get("summary")) or text(entry.get("details"))
        return Advisory(ident, summary, self.name, aliases)

    def query(self, packages: list) -> dict:
        packages = list(dict.fromkeys(packages))
        for ecosystem, _, _ in packages:
            if ecosystem not in ECOSYSTEMS:
                raise AdvisoryError(f"ecosystem {ecosystem!r} is not one osv.dev is asked about")
        found = self.ids(packages) if packages else {}
        cache, answer = {}, {}
        for key in packages:
            advisories = []
            try:
                for ident in found[key]:
                    if ident not in cache:
                        try:
                            cache[ident] = self.details(ident)
                        except AdvisoryError as exc:
                            cache[ident] = exc
                    if isinstance(cache[ident], AdvisoryError):
                        raise cache[ident]
                    if cache[ident] is not None:
                        advisories.append(cache[ident])
            except AdvisoryError as exc:
                answer[key] = exc
                continue
            answer[key] = sorted(advisories, key=lambda a: a.id)
        return answer
