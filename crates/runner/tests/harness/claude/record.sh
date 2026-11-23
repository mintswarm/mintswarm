#!/usr/bin/env bash
# Record Claude Code's headless behaviour for the runner's claude adapter, and
# derive the content-free fixtures its tests replay.
#
#   record.sh record [--raw DIR] [--out DIR] [--effort-args ARGS]
#   record.sh derive RAW [--out DIR]
#   record.sh compare A B
#   record.sh leak-check DIR
#   record.sh self-test
#
# record runs every scenario against the CLI in print mode with streaming JSON
# output: complete, needs-input, missing-result, corrective (a resume of
# missing-result's session), options (appended system prompt, opus, effort
# high, max turns 2, tools restricted and pre-approved to Read), effort-low
# (sonnet, effort low) and interrupt (SIGINT once the first stream line
# appears, SIGTERM 10 s later if the CLI is still running, SIGKILL 10 s after
# that). It also runs the probe commands: claude --help, claude --version and,
# when --help lists an auth command, claude auth status --json and
# claude auth status.
#
# The recording is isolated from the operator's session; the operator's login
# is used for authentication only. Each scenario runs in a fresh directory
# under ${TMPDIR:-/tmp}, outside the repository, with an environment built
# from empty: HOME, PATH, USER, LANG, TERM, TMPDIR, the variables named in
# MSW_HARNESS_ENV_PASS, and ANTHROPIC_API_KEY only when the credential mode is
# api_key. --setting-sources project,local leaves out the user source, so no
# user settings, user CLAUDE.md, memory or plugins load, and
# --strict-mcp-config with an empty --mcp-config loads no MCP server. Each
# flag is used only when --help lists it; versions.toml records, as
# isolation_settings, isolation_mcp, isolation_plugins and isolation_workdir,
# whether it was listed and whether the init events and the directories
# agree (no CLAUDE.md in the working directories or their parents, none in a
# git work tree, no MCP server, no plugin), with confirmed = false and the
# evidence when not. Prompts go to the CLI on standard input.
#
# Raw recordings are never committed. record writes them to RAW, by default
# build/harness/claude/raw under the top of the git work tree (git-ignored),
# and refuses a RAW inside the work tree but outside its build/ directory.
# record then runs derive on RAW, which writes to DIR (by default this
# script's directory) only what the runner's tests need, and no text from the
# session or the account:
#   fixtures/<scenario>.jsonl  one line per stream event, field names and
#       structure kept. Every free-text value (assistant text, results,
#       thinking, tool inputs and outputs, prompts, paths) becomes its length,
#       <text:N>; ids become <id:N>, numbered by first appearance across the
#       run so a resumed session keeps its id; times become <time> or 0; the
#       system/init event keeps counts (tools, MCP servers, slash commands,
#       plugins), never names. Kept as they are: numbers, booleans, enum values
#       the adapter relies on (type, subtype, role, stop_reason, status,
#       permission mode, rate-limit and login state), model names, the CLI
#       version and built-in tool names.
#   fixtures/probe.txt  the probe commands with their exit status and output
#       made content-free the same way, and the credential mode with where it
#       was read from.
#   versions.toml  the version, with { flag, confirmed, evidence } for each
#       requirement and isolation measure, the interrupt behaviour and the
#       rate-limit observation.
# The derived files are written to a temporary directory and leak-checked
# there; they are copied to DIR only when the leak check passes.
#
# derive re-derives DIR from an existing RAW. compare prints, for every
# scenario, whether the event kinds of A and B agree: a kind is an event's
# type, or type/subtype when it has a subtype; kinds containing rate_limit and
# system kinds other than system/init are dropped, and consecutive repeats
# collapse into one. A and B are directories holding fixtures/ or the
# *.jsonl files themselves. leak-check checks DIR/fixtures (or DIR when it
# has no fixtures/) against an allowlist: every string value must be a
# placeholder or an allowlisted enum, model, version or tool name, every field
# name an identifier, every probe.txt line one of the shapes record writes,
# and no other file may be there. It then fails when any file in DIR holds
# $HOME, the user name, the host name, an e-mail address or an API key.
# self-test runs record against a fake claude whose stream carries MCP server
# and plugin names, a CLAUDE.md-like passage, an e-mail address and paths,
# then compare and leak-check, in temporary directories only.
#
# Options:
#   --effort-args ARGS  when --help lists no --effort flag, the arguments that
#                       pass effort, with {effort} standing for the level, for
#                       example '--settings {"effortLevel":"{effort}"}'.
#                       Split on whitespace.
#
# Environment:
#   CLAUDE_BIN              the CLI to run (default claude)
#   MSW_HARNESS_CREDENTIAL  subscription (default) or api_key
#   MSW_HARNESS_ENV_PASS    extra variable names to pass, space-separated
#   MSW_HARNESS_INJECT      self-test only: a line appended to the derived
#                           complete.jsonl before the leak check, to prove
#                           that a value off the allowlist stops the write
#
# Exits 0 on success; 1 when a requirement is not confirmed, the leak check
# fails, compare finds a difference or the self-test fails; 2 on a usage or
# tool error. Needs bash, python3 and, for record, the CLI.
set -euo pipefail

prog="record.sh"
script_path="${BASH_SOURCE[0]}"
script_dir="$(cd "$(dirname "$script_path")" && pwd -P)"
script_path="$script_dir/$(basename "$script_path")"
stage_marker="MSW-STAGE-BODY-OK"
note_marker="msw-note-7f3a"
scenarios=(complete needs-input missing-result corrective options effort-low interrupt)

cleanup_paths=()
cleanup() {
    local path
    for path in ${cleanup_paths[@]+"${cleanup_paths[@]}"}; do
        rm -rf "$path"
    done
}
trap cleanup EXIT

note() { printf '%s: %s\n' "$prog" "$1" >&2; }
die() {
    note "$1"
    exit 2
}
usage() { die "usage: record.sh record [--raw DIR] [--out DIR] [--effort-args ARGS] | derive RAW [--out DIR] | compare A B | leak-check DIR | self-test"; }

temp_dir() { mktemp -d "${TMPDIR:-/tmp}/msw-claude.XXXXXX"; }

py_source() {
    cat <<'PY'
import json
import os
import re
import socket
import sys

SCENARIOS = ["complete", "needs-input", "missing-result", "corrective", "options", "effort-low", "interrupt"]
EMAIL_RE = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}")

# The allowlist. A fixture's string values may only be placeholders or values
# that pass kept(); its field names may only pass key_ok() or be placeholders.
PLACEHOLDER_RE = re.compile(r"<(?:text|key|id):\d+>|<time>")
TOKEN_RE = re.compile(r"[A-Za-z][A-Za-z0-9_]{0,39}(?:\.[a-z]{2,10})?")
MODEL_RE = re.compile(r"claude-[a-z0-9.-]{1,60}(?:\[1m\])?|haiku|sonnet|opus|<synthetic>")
VERSION_RE = re.compile(r"\d+\.\d+\.\d+")
TOOL_RE = re.compile(r"[A-Z][A-Za-z]{1,39}")
KEY_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]{0,63}")
TIME_RE = re.compile(r"timestamp|time|[A-Za-z_]*At|[a-z_]*_at")
ID_KEYS = {
    "id", "uuid", "session_id", "tool_use_id", "parent_tool_use_id", "request_id", "requestId", "message_id",
    "parent_uuid", "parentUuid", "agent_id", "task_id",
}
# Fields whose values are enums the adapter relies on: event kinds, roles,
# stop reasons, permission and credential modes, rate-limit and login state.
ENUM_KEYS = {
    "type", "subtype", "role", "stop_reason", "permissionMode", "apiKeySource", "status", "rateLimitType",
    "overageStatus", "overageDisabledReason", "terminal_reason", "service_tier", "error", "authMethod",
    "apiProvider", "subscriptionType", "method",
}
TOOL_USE_TYPES = {"tool_use", "server_tool_use"}
LABELS = {
    "login method", "auth method", "authentication method", "logged in", "status", "api provider",
    "subscription", "subscription type", "plan", "organization", "organisation", "org", "email", "account", "user",
}
VALUE_LABELS = {
    "login method", "auth method", "authentication method", "logged in", "status", "api provider",
    "subscription", "subscription type", "plan",
}
PROBE_HEADER = [
    "# Claude Code probe commands, written by record.sh. Content-free: every",
    "# free-text value is replaced by its length, <text:N>; field names, labels,",
    "# exit statuses, the version and enum values such as the login method are kept.",
]
PROBE_RES = [
    re.compile(x)
    for x in (
        r"",
        r"# recorded on \d{4}-\d{2}-\d{2}",
        r"== (?:claude --version|claude auth status --json|claude auth status|credential mode) ==",
        r"exit status: (?:-?\d+|skipped)(?: \(not available\))?",
        r"not available: --help lists no auth command",
        r"\d+\.\d+\.\d+ \(Claude Code\)",
        r"<text:\d+>",
        r"apiKeySource in the complete scenario's system/init event: (?:not reported|<text:\d+>|"
        + TOKEN_RE.pattern
        + r")",
        r"ANTHROPIC_API_KEY passed to the CLI: (?:yes|no)",
        r"logged in \(from claude auth status --json\): (?:true|false|not reported)",
        r"credential mode \(inferred from apiKeySource and ANTHROPIC_API_KEY, not reported by the CLI\): "
        r"(?:subscription|api_key)",
    )
]
LABEL_LINE_RE = re.compile(r"([A-Za-z][A-Za-z ]{0,30}): (.*)")


def placeholder(s):
    return "<text:%d>" % len(s)


def kept(key, parent, s):
    """Whether a string value is on the allowlist as it stands."""
    if key in ENUM_KEYS:
        return TOKEN_RE.fullmatch(s) is not None
    if key == "model":
        return MODEL_RE.fullmatch(s) is not None
    if key == "claude_code_version":
        return VERSION_RE.fullmatch(s) is not None
    if key == "name" and isinstance(parent, dict) and parent.get("type") in TOOL_USE_TYPES:
        return TOOL_RE.fullmatch(s) is not None
    return False


def key_ok(k):
    return KEY_RE.fullmatch(k) is not None or MODEL_RE.fullmatch(k) is not None


def is_time(key):
    return isinstance(key, str) and TIME_RE.fullmatch(key) is not None


def is_init(ev):
    return isinstance(ev, dict) and ev.get("type") == "system" and ev.get("subtype") == "init"


class Deriver:
    """Turns a raw event into a content-free one. Ids are numbered by first
    appearance across the whole run, so a resumed session keeps its id."""

    def __init__(self):
        self.ids = {}

    def ident(self, s):
        if s not in self.ids:
            self.ids[s] = "<id:%d>" % (len(self.ids) + 1)
        return self.ids[s]

    def key(self, k):
        return k if key_ok(k) else "<key:%d>" % len(k)

    def event(self, ev):
        if not is_init(ev):
            return self.walk(ev, None, None)
        # The init event keeps counts, never names.
        return {
            self.key(k): len(x) if isinstance(x, (list, dict)) and k not in ("type", "subtype") else self.walk(x, k, ev)
            for k, x in ev.items()
        }

    def walk(self, v, key, parent):
        if isinstance(v, dict):
            return {self.key(k): self.walk(x, k, v) for k, x in v.items()}
        if isinstance(v, list):
            return [self.walk(x, key, parent) for x in v]
        if isinstance(v, bool) or v is None:
            return v
        if isinstance(v, (int, float)):
            return 0 if is_time(key) else v
        if isinstance(v, str):
            if key in ID_KEYS:
                return self.ident(v)
            if is_time(key):
                return "<time>"
            if kept(key, parent, v):
                return v
            return placeholder(v)
        return placeholder(str(v))


def safe(s, regex):
    s = "" if s is None else str(s)
    return s if regex.fullmatch(s) else placeholder(s)


def read(path, default=""):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        return default


def load_events(path):
    events = []
    for line in read(path).splitlines():
        if not line.strip():
            continue
        try:
            events.append(json.loads(line))
        except ValueError:
            events.append(None)
    return events


def kind(ev):
    if not isinstance(ev, dict):
        return "non-json"
    t = str(ev.get("type", "untyped"))
    sub = ev.get("subtype")
    return "%s/%s" % (t, sub) if sub else t


def kinds(events):
    out = []
    for k in map(kind, events):
        if "rate_limit" in k:
            continue
        if k.startswith("system") and k != "system/init":
            continue
        if not out or out[-1] != k:
            out.append(k)
    return out


# The leak check: an allowlist over every fixture, then a denylist of the
# operator's own strings over every derived file.
def json_problems(v, key, parent, where, problems):
    if isinstance(v, dict):
        for k, x in v.items():
            if not (key_ok(k) or PLACEHOLDER_RE.fullmatch(k)):
                problems.append("%s: a field name of %d characters is not allowlisted" % (where, len(k)))
            json_problems(x, k, v, where, problems)
    elif isinstance(v, list):
        for x in v:
            json_problems(x, key, parent, where, problems)
    elif isinstance(v, str):
        if not (PLACEHOLDER_RE.fullmatch(v) or kept(key, parent, v)):
            problems.append("%s: the value of %s (%d characters) is not allowlisted" % (where, key, len(v)))
    elif not (v is None or isinstance(v, (bool, int, float))):
        problems.append("%s: a value of type %s is not allowlisted" % (where, type(v).__name__))


def json_line_problems(line, where, problems):
    try:
        data = json.loads(line)
    except ValueError:
        problems.append("%s: not JSON" % where)
        return
    json_problems(data, None, None, where, problems)


def probe_line_ok(line):
    if line in PROBE_HEADER or any(r.fullmatch(line) for r in PROBE_RES):
        return True
    m = LABEL_LINE_RE.fullmatch(line)
    if not m or m.group(1).lower() not in LABELS:
        return False
    value = m.group(2)
    if re.fullmatch(r"<text:\d+>", value):
        return True
    return m.group(1).lower() in VALUE_LABELS and TOKEN_RE.fullmatch(value) is not None


def allowlist_problems(fixtures, problems):
    for name in sorted(os.listdir(fixtures)):
        path = os.path.join(fixtures, name)
        if os.path.isdir(path) or not (name.endswith(".jsonl") or name == "probe.txt"):
            problems.append("%s: not a fixture file" % name)
            continue
        for i, line in enumerate(read(path).splitlines(), 1):
            where = "%s:%d" % (name, i)
            if name.endswith(".jsonl"):
                json_line_problems(line, where, problems)
            elif line.startswith("{"):
                json_line_problems(line, where, problems)
            elif not probe_line_ok(line):
                problems.append("%s: line is not allowlisted" % where)


def boundary(word):
    return re.compile(r"(?<![A-Za-z0-9])" + re.escape(word) + r"(?![A-Za-z0-9])")


def denylist_problems(directory, meta, problems):
    lits = []
    for label, value in (
        ("scratch directory", meta.get("scratch_physical", "")),
        ("scratch directory", meta.get("scratch", "")),
        ("home directory", meta.get("home", "")),
        ("API key", "sk-ant-"),
        ("API key", os.environ.get("ANTHROPIC_API_KEY", "")),
    ):
        if len(value) > 3:
            lits.append((label, value))
    words = [(label, boundary(v)) for label, v in (("user name", meta.get("user", "")), ("host name", meta.get("host", ""))) if v]
    for root, _, files in os.walk(directory):
        for name in sorted(files):
            path = os.path.join(root, name)
            rel = os.path.relpath(path, directory)
            for i, line in enumerate(read(path).splitlines(), 1):
                for label, lit in lits:
                    if lit in line:
                        problems.append("%s:%d: %s" % (rel, i, label))
                for label, pat in words:
                    if pat.search(line):
                        problems.append("%s:%d: %s" % (rel, i, label))
                if EMAIL_RE.search(line):
                    problems.append("%s:%d: e-mail address" % (rel, i))


def leak_check(directory, meta):
    problems = []
    fixtures = os.path.join(directory, "fixtures")
    allowlist_problems(fixtures if os.path.isdir(fixtures) else directory, problems)
    denylist_problems(directory, meta, problems)
    for p in problems:
        print("record.sh: leak: " + p, file=sys.stderr)
    return not problems


def read_meta(raw):
    meta = {}
    for line in read(os.path.join(raw, "run.txt")).splitlines():
        k, _, v = line.partition("=")
        meta[k] = v
    return meta


def effort_key(v):
    if isinstance(v, dict):
        return any("effort" in str(k).lower() or effort_key(x) for k, x in v.items())
    if isinstance(v, list):
        return any(effort_key(x) for x in v)
    return False


def q(s):
    return json.dumps(s, ensure_ascii=False)


def b(v):
    return "true" if v else "false"


def write_lines(path, lines):
    with open(path, "w", encoding="utf-8") as f:
        for line in lines:
            f.write(line.rstrip() + "\n")


def probe_text_line(line):
    line = line.rstrip()
    if not line:
        return ""
    m = LABEL_LINE_RE.fullmatch(line)
    if m and m.group(1).lower() in LABELS:
        value = m.group(2).strip()
        if m.group(1).lower() in VALUE_LABELS and TOKEN_RE.fullmatch(value):
            return "%s: %s" % (m.group(1), value)
        return "%s: %s" % (m.group(1), placeholder(value))
    return placeholder(line)


def derive(raw, stage):
    meta = read_meta(raw)
    if not meta:
        print("record.sh: %s has no run.txt; not a raw recording" % raw, file=sys.stderr)
        return 2
    der = Deriver()
    help_text = read(os.path.join(raw, "help.out"))

    def listed(flag):
        return bool(flag) and re.search(r"(?<![A-Za-z0-9-])" + re.escape(flag) + r"(?![A-Za-z0-9-])", help_text) is not None

    def status(s):
        try:
            return int(read(os.path.join(raw, s + ".status")).strip())
        except ValueError:
            return None

    events = {s: load_events(os.path.join(raw, s + ".jsonl")) for s in SCENARIOS}
    fixtures = os.path.join(stage, "fixtures")
    os.makedirs(fixtures, exist_ok=True)
    for s in SCENARIOS:
        lines = []
        for ev in events[s]:
            out = der.event(ev) if isinstance(ev, dict) else {"type": "non-json"}
            lines.append(json.dumps(out, ensure_ascii=False))
        write_lines(os.path.join(fixtures, s + ".jsonl"), lines)

    dicts = {s: [e for e in events[s] if isinstance(e, dict)] for s in SCENARIOS}

    def init(s):
        return next((e for e in dicts[s] if is_init(e)), None)

    def result(s):
        return next((e for e in reversed(dicts[s]) if e.get("type") == "result"), None)

    def sid(s):
        for e in (init(s), result(s)):
            if e and e.get("session_id"):
                return e["session_id"]
        return None

    def result_text(s):
        r = result(s)
        return r.get("result") if r and isinstance(r.get("result"), str) else ""

    def init_model(s):
        i = init(s)
        return i.get("model") if i and isinstance(i.get("model"), str) else ""

    def turns(s):
        r = result(s)
        return r.get("num_turns") if r and isinstance(r.get("num_turns"), int) else None

    def yn(v):
        return "yes" if v else "no"

    req = {}
    unconfirmed = []

    def put(key, flag, ok, evidence):
        req[key] = (flag, bool(ok), evidence)
        if not ok:
            unconfirmed.append(key)

    sf = meta.get("stream_flags", "")
    ok = listed("--output-format") and "stream-json" in help_text and listed("--verbose")
    put("stream_json", sf, ok and status("complete") == 0 and init("complete") and result("complete"),
        "--output-format, stream-json and --verbose listed in --help: %s; complete exited %s with system/init: %s, result: %s"
        % (yn(ok), status("complete"), yn(init("complete")), yn(result("complete"))))

    f = meta.get("append_flag", "")
    marker = meta.get("stage_marker", "")
    has = bool(marker) and marker in result_text("options")
    put("append_system_prompt", f, listed(f) and status("options") == 0 and has,
        "listed in --help: %s; options exited %s; its reply starts with the marker the appended prompt asks for: %s"
        % (yn(listed(f)), status("options"), yn(has)))

    f = meta.get("model_flag", "")
    pairs = [("complete", "haiku"), ("options", "opus"), ("effort-low", "sonnet")]
    agree = all(status(s) == 0 and alias in init_model(s).lower() for s, alias in pairs)
    put("model", f, listed(f) and agree,
        "listed in --help: %s; system/init model for %s"
        % (yn(listed(f)), ", ".join("%s (%s) = %s, exit %s" % (s, a, safe(init_model(s), MODEL_RE) if init_model(s) else "none", status(s)) for s, a in pairs)))

    mode = meta.get("effort_mode", "unsupported")
    f = meta.get("effort_flag", "")
    echoed = any(effort_key(e) for s in ("options", "effort-low") for e in dicts[s])
    ran = status("options") == 0 and status("effort-low") == 0
    if mode == "flag":
        put("effort", f, listed(f) and ran,
            "listed in --help: %s; options (high) exited %s, effort-low (low) exited %s; the stream %s effort"
            % (yn(listed(f)), status("options"), status("effort-low"), "mentions" if echoed else "does not report"))
    elif mode == "args":
        put("effort", f, ran,
            "no --effort in --help; passed with --effort-args; options (high) exited %s, effort-low (low) exited %s; the stream %s effort"
            % (status("options"), status("effort-low"), "mentions" if echoed else "does not report"))
    else:
        put("effort", "unsupported", False, "no --effort in --help and no --effort-args given")

    f = meta.get("max_turns_flag", "")
    t1, t2 = turns("complete"), turns("options")
    ok = status("complete") == 0 and status("options") == 0 and t1 is not None and t1 <= 1 and t2 is not None and t2 <= 2
    put("max_turns", f, listed(f) and ok,
        "listed in --help: %s; complete (limit 1) num_turns = %s; options (limit 2) num_turns = %s"
        % (yn(listed(f)), t1, t2))

    f = meta.get("allowed_flag", "")
    restrict = meta.get("restrict_flag", "")
    read_ids = set()
    for e in dicts["options"]:
        if e.get("type") == "assistant":
            for c in (e.get("message") or {}).get("content") or []:
                if isinstance(c, dict) and c.get("type") == "tool_use" and c.get("name") == "Read":
                    read_ids.add(c.get("id"))
    read_ok = False
    for e in dicts["options"]:
        if e.get("type") == "user":
            for c in (e.get("message") or {}).get("content") or []:
                if isinstance(c, dict) and c.get("type") == "tool_result" and c.get("tool_use_id") in read_ids:
                    read_ok = read_ok or not c.get("is_error")
    tools = (init("options") or {}).get("tools")
    only_read = tools == ["Read"]
    shown = json.dumps([safe(x, TOOL_RE) for x in tools]) if isinstance(tools, list) else "not reported"
    note_seen = meta.get("note_marker", "") in result_text("options")
    ok = listed(f) and status("options") == 0 and read_ok and note_seen and (only_read if restrict else True)
    put("allowed_tools", f, ok,
        "listed in --help: %s; options exited %s; Read ran without error: %s; the reply quotes the file: %s; %s"
        % (yn(listed(f)), status("options"), yn(read_ok), yn(note_seen),
           "with %s Read the init tool list is %s" % (restrict, shown) if restrict else "no flag restricts the tool set"))

    f = meta.get("resume_flag", "")
    same = sid("corrective") is not None and sid("corrective") == sid("missing-result")
    fixed = "msw-result" in result_text("corrective")
    put("resume", f, listed(f) and status("corrective") == 0 and same and fixed,
        "listed in --help: %s; corrective exited %s; its session id equals missing-result's: %s; its reply has the msw-result block: %s"
        % (yn(listed(f)), status("corrective"), yn(same), yn(fixed)))

    # Isolation from the operator's settings, CLAUDE.md, memory, MCP servers
    # and plugins, checked against --help and the init events.
    inits = [init(s) for s in SCENARIOS if init(s)]

    def counts(field):
        return [len(i[field]) for i in inits if isinstance(i.get(field), (list, dict))]

    found = meta.get("instruction_files", "")
    ss = meta.get("settings_flag", "")
    ok = bool(ss) and listed("--setting-sources") and found == "0" and bool(inits)
    put("isolation_settings", ss or "--setting-sources", ok,
        "listed in --help: %s; the user source, which holds the user settings, CLAUDE.md and memory, is left out: %s; "
        "CLAUDE.md, CLAUDE.local.md or .claude/CLAUDE.md in the working directories or their parents: %s"
        % (yn(listed("--setting-sources")), yn(bool(ss)), found or "not checked"))

    mc = counts("mcp_servers")
    strict = meta.get("mcp_flag", "")
    ok = bool(strict) and listed("--strict-mcp-config") and listed("--mcp-config") and bool(mc) and max(mc) == 0
    put("isolation_mcp", strict or "--strict-mcp-config", ok,
        "--strict-mcp-config and --mcp-config listed in --help: %s; MCP servers reported by system/init: %s"
        % (yn(listed("--strict-mcp-config") and listed("--mcp-config")),
           "the highest count is %d over %d scenarios" % (max(mc), len(mc)) if mc else "not reported"))

    pc = counts("plugins")
    put("isolation_plugins", ss or "--setting-sources", bool(ss) and bool(pc) and max(pc) == 0,
        "plugins are enabled only from settings, and the user source is left out: %s; plugins reported by system/init: %s"
        % (yn(bool(ss)), "the highest count is %d over %d scenarios" % (max(pc), len(pc)) if pc else "not reported"))

    in_git = meta.get("workdir_in_git", "")
    put("isolation_workdir", "", in_git == "no",
        "every scenario ran in a fresh directory under TMPDIR; inside a git work tree: %s" % (in_git or "not checked"))

    # probe.txt
    version_out = read(os.path.join(raw, "version.out")).strip()
    m = re.search(r"\d+\.\d+\.\d+", version_out)
    version = m.group(0) if m else "unknown"
    if version == "unknown":
        unconfirmed.append("version")
    probe = PROBE_HEADER + [
        "# recorded on %s" % meta.get("recorded_on", ""),
        "",
        "== claude --version ==",
        "exit status: %s" % read(os.path.join(raw, "version.status")).strip(),
    ]
    probe += [l if re.fullmatch(r"\d+\.\d+\.\d+ \(Claude Code\)", l) else placeholder(l) for l in version_out.splitlines()]

    logged = None
    for name, argv in (("auth-json", "claude auth status --json"), ("auth-text", "claude auth status")):
        probe += ["", "== %s ==" % argv]
        if meta.get("auth_listed") != "yes":
            probe.append("not available: --help lists no auth command")
            continue
        st = read(os.path.join(raw, name + ".status")).strip()
        out = read(os.path.join(raw, name + ".out"))
        probe.append("exit status: %s%s" % (st, "" if st == "0" else " (not available)"))
        data = None
        if name == "auth-json":
            try:
                data = json.loads(out)
            except ValueError:
                data = None
        if isinstance(data, dict):
            probe.append(json.dumps(der.walk(data, None, None), ensure_ascii=False))
            if st == "0" and isinstance(data.get("loggedIn"), bool):
                logged = data["loggedIn"]
        else:
            probe += [probe_text_line(l) for l in out.splitlines()]
    source = (init("complete") or {}).get("apiKeySource")
    passed = meta.get("api_key_passed") == "yes"
    if passed:
        mode_value = "api_key"
    elif source in (None, "none"):
        mode_value = "subscription"
    else:
        mode_value = "api_key"
    probe += [
        "",
        "== credential mode ==",
        "apiKeySource in the complete scenario's system/init event: %s" % ("not reported" if source is None else safe(source, TOKEN_RE)),
        "ANTHROPIC_API_KEY passed to the CLI: %s" % ("yes" if passed else "no"),
        "logged in (from claude auth status --json): %s" % ("not reported" if logged is None else b(logged)),
        "credential mode (inferred from apiKeySource and ANTHROPIC_API_KEY, not reported by the CLI): %s" % mode_value,
    ]
    write_lines(os.path.join(fixtures, "probe.txt"), probe)

    # versions.toml
    rate = [s for s in SCENARIOS if any("rate_limit" in kind(e) for e in dicts[s])]
    intr = result("interrupt") is not None
    try:
        intr_status = int(read(os.path.join(raw, "interrupt.status")).strip())
    except ValueError:
        intr_status = -1
    vt = [
        "# Claude Code versions validated for the runner's claude adapter, written by",
        "# record.sh from a recording; re-run it rather than editing by hand.",
        "#",
        "# Each requirement is { flag, confirmed, evidence }. confirmed means the flag is",
        "# listed in claude --help, the scenarios that use it exited 0 and, where the",
        "# stream shows the setting, the stream agrees. effort may be \"unsupported\".",
        "# The isolation_* entries record how the recording was kept apart from the",
        "# operator's settings, CLAUDE.md, memory, MCP servers and plugins.",
        "#",
        "# Event-kind comparison rule (record.sh compare): an event's kind is its type,",
        "# or type/subtype when it has a subtype; kinds containing rate_limit and system",
        "# kinds other than system/init are dropped, and consecutive repeats of a kind",
        "# collapse into one. Two recordings agree when every scenario has the same",
        "# sequence of kinds.",
        "#",
        "# fixtures/ holds only content-free files derived from the recording: free",
        "# text is replaced by its length, ids and times are normalised, the init event",
        "# keeps counts only. Raw recordings are never committed.",
        "",
        "[claude.%s]" % q(version),
        "recorded_on = %s" % q(meta.get("recorded_on", "")),
    ]
    for key in (
        "stream_json", "append_system_prompt", "model", "effort", "max_turns", "allowed_tools", "resume",
        "isolation_settings", "isolation_mcp", "isolation_plugins", "isolation_workdir",
    ):
        flag, ok, ev = req[key]
        vt.append("%s = { flag = %s, confirmed = %s, evidence = %s }" % (key, q(flag), b(ok), q(ev)))
    perm = meta.get("permission_flag", "")
    vt += [
        "restrict_tools = { flag = %s, listed = %s }" % (q(restrict or "--tools"), b(bool(restrict))),
        "permission_mode = { flag = %s, listed = %s, value = %s }" % (q(perm or "--permission-mode"), b(bool(perm)), q("default" if perm else "")),
        "interrupt = { signal = %s, exit_status = %d, result_event = %s }" % (q(meta.get("interrupt_signal", "")), intr_status, b(intr)),
        "rate_limit_event = %s" % ("[%s]" % ", ".join(q("fixtures/%s.jsonl" % s) for s in rate) if rate else q("not observed")),
        "credential_mode = { value = %s, source = %s }" % (q(mode_value), q("inferred from apiKeySource and whether ANTHROPIC_API_KEY was passed")),
    ]
    write_lines(os.path.join(stage, "versions.toml"), vt)

    if unconfirmed:
        print("record.sh: not confirmed: " + ", ".join(unconfirmed), file=sys.stderr)
        return 1
    return 0


def fixture_dir(d):
    f = os.path.join(d, "fixtures")
    return f if os.path.isdir(f) else d


def compare(a, b_dir):
    da, db = fixture_dir(a), fixture_dir(b_dir)
    names = sorted({n[:-6] for d in (da, db) if os.path.isdir(d) for n in os.listdir(d) if n.endswith(".jsonl")})
    if not names:
        print("record.sh: no *.jsonl in %s or %s" % (da, db), file=sys.stderr)
        return 1
    same_all = True
    for n in names:
        pa, pb = os.path.join(da, n + ".jsonl"), os.path.join(db, n + ".jsonl")
        if not (os.path.isfile(pa) and os.path.isfile(pb)):
            print("%s: missing in %s" % (n, a if not os.path.isfile(pa) else b_dir))
            same_all = False
            continue
        ka, kb = kinds(load_events(pa)), kinds(load_events(pb))
        if ka == kb:
            print("%s: same: %s" % (n, " ".join(ka)))
        else:
            same_all = False
            print("%s: differs" % n)
            print("  A: %s" % " ".join(ka))
            print("  B: %s" % " ".join(kb))
    return 0 if same_all else 1


def session_id(path):
    for e in load_events(path):
        if isinstance(e, dict) and e.get("session_id"):
            print(e["session_id"])
            return 0
    return 1


def main(argv):
    mode = argv[0]
    if mode == "derive":
        return derive(argv[1], argv[2])
    if mode == "compare":
        return compare(argv[1], argv[2])
    if mode == "session-id":
        return session_id(argv[1])
    if mode == "leak-check":
        if len(argv) > 2:
            meta = read_meta(argv[2])
        else:
            meta = {"home": os.environ.get("HOME", ""), "user": os.environ.get("USER", ""), "host": socket.gethostname().split(".")[0]}
        return 0 if leak_check(argv[1], meta) else 1
    if mode == "realpath":
        print(os.path.realpath(argv[1]))
        return 0
    return 2


sys.exit(main(sys.argv[1:]))
PY
}

py() { python3 -I -c "$(py_source)" "$@"; }

require_python() {
    command -v python3 >/dev/null 2>&1 || die "python3 is not on PATH"
}

# Copy a derived stage directory into the output directory.
publish() {
    local stage="$1" out="$2" name
    mkdir -p "$out/fixtures" || die "cannot create $out/fixtures"
    cp "$stage/versions.toml" "$out/versions.toml" || die "cannot write $out/versions.toml"
    for name in "${scenarios[@]}"; do
        cp "$stage/fixtures/$name.jsonl" "$out/fixtures/$name.jsonl" || die "cannot write $out/fixtures"
    done
    cp "$stage/fixtures/probe.txt" "$out/fixtures/probe.txt" || die "cannot write $out/fixtures"
}

# Derive RAW into a temporary directory, leak-check it and copy it to OUT.
run_derive() {
    local raw="$1" out="$2" stage status=0
    stage="$(temp_dir)" || die "mktemp failed"
    cleanup_paths+=("$stage")
    py derive "$raw" "$stage" || status=$?
    case "$status" in
        0 | 1) ;;
        *) die "derive failed on $raw" ;;
    esac
    if [ -n "${MSW_HARNESS_INJECT:-}" ]; then
        printf '%s\n' "$MSW_HARNESS_INJECT" >>"$stage/fixtures/complete.jsonl"
    fi
    if ! py leak-check "$stage" "$raw"; then
        note "leak check failed; nothing was written to $out"
        return 1
    fi
    publish "$stage" "$out"
    note "derived fixtures written to $out"
    return "$status"
}

listed() { grep -Eq -- "(^|[^A-Za-z0-9-])$1([^A-Za-z0-9-]|\$)" "$raw/help.out"; }

# Prints how many CLAUDE.md, CLAUDE.local.md or .claude/CLAUDE.md files sit
# in DIR or any of its parents, where Claude Code would load them from.
count_instruction_files() {
    local dir="$1" n=0 file
    while :; do
        for file in CLAUDE.md CLAUDE.local.md .claude/CLAUDE.md; do
            if [ -e "$dir/$file" ]; then
                n=$((n + 1))
            fi
        done
        if [ "$dir" = / ] || [ -z "$dir" ]; then
            break
        fi
        dir="$(dirname "$dir")"
    done
    printf '%s\n' "$n"
}

# run_scenario NAME DIR PROMPT_FILE ARGS...
run_scenario() {
    local name="$1" dir="$2" prompt="$3" status=0
    shift 3
    printf '%s\n' "$@" >"$raw/$name.args"
    (cd "$dir" && exec env -i "${env_args[@]}" "$claude_bin" "$@") <"$prompt" >"$raw/$name.jsonl" 2>"$raw/$name.stderr" || status=$?
    printf '%s\n' "$status" >"$raw/$name.status"
    note "$name: exit $status"
}

# probe NAME ARGS...
probe() {
    local name="$1" status=0
    shift
    (cd "$scratch" && exec env -i "${env_args[@]}" "$claude_bin" "$@") </dev/null >"$raw/$name.out" 2>&1 || status=$?
    printf '%s\n' "$status" >"$raw/$name.status"
}

# Sets effort_args for a level: the --effort flag, the --effort-args
# template, or nothing.
effort_for() {
    effort_args=()
    case "$effort_mode" in
        flag) effort_args=(--effort "$1") ;;
        args) read -r -a effort_args <<<"${effort_template//\{effort\}/$1}" ;;
    esac
}

write_prompts() {
    local dir="$1" fence='```'
    mkdir -p "$dir"
    cat >"$dir/complete.txt" <<EOF
msw-scenario: complete
Reply with exactly the lines between BEGIN and END, without those two lines, and nothing else.
BEGIN
Done.

${fence}yaml
msw-result:
  status: complete
  questions: []
${fence}
END
EOF
    cat >"$dir/needs-input.txt" <<EOF
msw-scenario: needs-input
Reply with exactly the lines between BEGIN and END, without those two lines, and nothing else.
BEGIN
One question is open.

${fence}yaml
msw-result:
  status: needs_input
  questions: ["Which colour should the button be?"]
${fence}
END
EOF
    cat >"$dir/missing-result.txt" <<'EOF'
msw-scenario: missing-result
Reply with exactly this line and nothing else: Working on it.
EOF
    cat >"$dir/corrective.txt" <<EOF
msw-scenario: corrective
Your previous reply had no msw-result block. Reply with exactly the lines between BEGIN and END, without those two lines, and nothing else.
BEGIN
${fence}yaml
msw-result:
  status: complete
  questions: []
${fence}
END
EOF
    cat >"$dir/options.txt" <<'EOF'
msw-scenario: options
Use the Read tool to read the file note.txt in the current directory, then reply with its first line only.
EOF
    cat >"$dir/effort-low.txt" <<'EOF'
msw-scenario: effort-low
Reply with exactly this line and nothing else: low effort ok
EOF
    cat >"$dir/interrupt.txt" <<'EOF'
msw-scenario: interrupt
Count from 1 to 3000, one number per line, with no other text.
EOF
}

# Runs the interrupt scenario: SIGINT once the first stream line appears,
# then SIGTERM and SIGKILL if the CLI is still running.
run_interrupt() {
    local dir="$1" prompt="$2" pid status=0 i
    shift 2
    printf '%s\n' "$@" >"$raw/interrupt.args"
    # Job control puts the CLI in its own process group with SIGINT not
    # ignored, as it is for background jobs of a non-interactive shell.
    set -m
    (cd "$dir" && exec env -i "${env_args[@]}" "$claude_bin" "$@") <"$prompt" >"$raw/interrupt.jsonl" 2>"$raw/interrupt.stderr" &
    pid=$!
    set +m
    i=0
    while [ "$i" -lt 600 ] && [ ! -s "$raw/interrupt.jsonl" ] && kill -0 "$pid" 2>/dev/null; do
        sleep 0.5
        i=$((i + 1))
    done
    interrupt_signal="none (exited before the first line)"
    if kill -0 "$pid" 2>/dev/null; then
        interrupt_signal="SIGINT"
        kill -INT "$pid" 2>/dev/null || true
        i=0
        while [ "$i" -lt 20 ] && kill -0 "$pid" 2>/dev/null; do
            sleep 0.5
            i=$((i + 1))
        done
        if kill -0 "$pid" 2>/dev/null; then
            interrupt_signal="SIGINT, then SIGTERM after 10 s"
            kill -TERM "$pid" 2>/dev/null || true
            i=0
            while [ "$i" -lt 20 ] && kill -0 "$pid" 2>/dev/null; do
                sleep 0.5
                i=$((i + 1))
            done
            if kill -0 "$pid" 2>/dev/null; then
                interrupt_signal="SIGINT, then SIGTERM after 10 s, then SIGKILL after 10 s"
                kill -KILL "$pid" 2>/dev/null || true
            fi
        fi
    fi
    wait "$pid" || status=$?
    printf '%s\n' "$status" >"$raw/interrupt.status"
    note "interrupt: $interrupt_signal, exit $status"
}

cmd_record() {
    local out="$script_dir" root default_raw=""
    raw=""
    effort_template=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --raw)
                [ "$#" -ge 2 ] || usage
                raw="$2"
                shift
                ;;
            --out)
                [ "$#" -ge 2 ] || usage
                out="$2"
                shift
                ;;
            --effort-args)
                [ "$#" -ge 2 ] || usage
                effort_template="$2"
                shift
                ;;
            *) usage ;;
        esac
        shift
    done
    require_python
    claude_bin="${CLAUDE_BIN:-claude}"
    command -v "$claude_bin" >/dev/null 2>&1 || die "$claude_bin is not on PATH (set CLAUDE_BIN)"
    credential="${MSW_HARNESS_CREDENTIAL:-subscription}"
    case "$credential" in
        subscription | api_key) ;;
        *) die "MSW_HARNESS_CREDENTIAL must be subscription or api_key" ;;
    esac

    root="$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || true)"
    if [ -n "$root" ]; then
        root="$(py realpath "$root")"
        default_raw="$root/build/harness/claude/raw"
    fi
    [ -n "$raw" ] || raw="$default_raw"
    [ -n "$raw" ] || die "not in a git work tree: give --raw DIR"
    raw="$(py realpath "$raw")"
    if [ -n "$root" ]; then
        case "$raw/" in
            "$root/build/"*) ;;
            "$root/"*) die "raw recordings are never kept in the work tree: give a --raw outside $root or under $root/build/" ;;
        esac
    fi
    mkdir -p "$raw" || die "cannot create $raw"
    out="$(py realpath "$out")"

    env_args=("HOME=$HOME" "PATH=$PATH" "USER=${USER:-}" "LANG=${LANG:-en_GB.UTF-8}" "TERM=${TERM:-dumb}" "TMPDIR=${TMPDIR:-/tmp}")
    local name api_key_passed="no"
    for name in ${MSW_HARNESS_ENV_PASS:-}; do
        case "$name" in
            *[!A-Za-z0-9_]* | [0-9]*) die "MSW_HARNESS_ENV_PASS: not a variable name: $name" ;;
        esac
        if [ -n "${!name+x}" ]; then
            env_args+=("$name=${!name}")
        fi
    done
    if [ "$credential" = api_key ]; then
        [ -n "${ANTHROPIC_API_KEY:-}" ] || die "MSW_HARNESS_CREDENTIAL=api_key needs ANTHROPIC_API_KEY"
        env_args+=("ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY")
        api_key_passed="yes"
    fi

    scratch="$(temp_dir)" || die "mktemp failed"
    cleanup_paths+=("$scratch")
    local scratch_physical
    scratch_physical="$(cd "$scratch" && pwd -P)"

    note "raw recording: $raw"
    probe help --help
    probe version --version
    local auth_listed="no"
    if grep -Eq '^[[:space:]]+auth([[:space:]]|\[|$)' "$raw/help.out"; then
        auth_listed="yes"
        probe auth-json auth status --json
        probe auth-text auth status
    fi

    local stream_flags="-p --output-format stream-json --verbose"
    local common=(-p --output-format stream-json --verbose)
    # Isolation: leave out the user setting source (user settings, CLAUDE.md,
    # memory and the plugins they enable) and load no MCP server.
    local settings_flag="" mcp_flag=""
    if listed --setting-sources; then
        common+=(--setting-sources "project,local")
        settings_flag="--setting-sources project,local"
    else
        note "isolation: --help lists no --setting-sources"
    fi
    if listed --strict-mcp-config && listed --mcp-config; then
        printf '%s\n' '{"mcpServers":{}}' >"$scratch/mcp.json"
        common+=(--strict-mcp-config --mcp-config "$scratch/mcp.json")
        mcp_flag="--strict-mcp-config --mcp-config (an empty mcpServers object)"
    else
        note "isolation: --help lists no --strict-mcp-config or --mcp-config"
    fi
    local instruction_files workdir_in_git="no"
    instruction_files="$(count_instruction_files "$scratch_physical")"
    if [ "$(git -C "$scratch_physical" rev-parse --is-inside-work-tree 2>/dev/null || true)" = true ]; then
        workdir_in_git="yes"
    fi

    effort_mode="unsupported"
    local effort_flag="unsupported"
    if listed --effort; then
        effort_mode="flag"
        effort_flag="--effort"
    elif [ -n "$effort_template" ]; then
        effort_mode="args"
        effort_flag="$effort_template"
    fi
    local allowed_flag="--allowedTools"
    if ! listed --allowedTools && listed --allowed-tools; then
        allowed_flag="--allowed-tools"
    fi
    local restrict=() restrict_flag=""
    if listed --tools; then
        restrict=(--tools Read)
        restrict_flag="--tools"
    fi
    local permission=() permission_flag=""
    if listed --permission-mode; then
        permission=(--permission-mode default)
        permission_flag="--permission-mode"
    fi

    local prompts="$scratch/prompts" dir s
    write_prompts "$prompts"
    for s in "${scenarios[@]}"; do
        mkdir -p "$scratch/$s"
    done
    printf 'MSW harness note.\n%s\n' "$note_marker" >"$scratch/options/note.txt"

    run_scenario complete "$scratch/complete" "$prompts/complete.txt" "${common[@]}" --model haiku --max-turns 1
    run_scenario needs-input "$scratch/needs-input" "$prompts/needs-input.txt" "${common[@]}" --model haiku --max-turns 1
    run_scenario missing-result "$scratch/missing-result" "$prompts/missing-result.txt" "${common[@]}" --model haiku --max-turns 1
    local session=""
    session="$(py session-id "$raw/missing-result.jsonl" || true)"
    if [ -n "$session" ]; then
        # A session is stored per working directory, so the resume runs in
        # missing-result's directory.
        run_scenario corrective "$scratch/missing-result" "$prompts/corrective.txt" "${common[@]}" --model haiku --max-turns 1 --resume "$session"
    else
        note "corrective: skipped, missing-result reported no session id"
        printf 'skipped\n' >"$raw/corrective.status"
        : >"$raw/corrective.jsonl"
    fi
    local stage_body="You are the mintswarm harness-experiment stage. Begin every reply with the line $stage_marker, then answer on the next line."
    effort_for high
    run_scenario options "$scratch/options" "$prompts/options.txt" "${common[@]}" --model opus ${effort_args[@]+"${effort_args[@]}"} \
        --append-system-prompt "$stage_body" --max-turns 2 ${permission[@]+"${permission[@]}"} ${restrict[@]+"${restrict[@]}"} "$allowed_flag" Read
    effort_for low
    run_scenario effort-low "$scratch/effort-low" "$prompts/effort-low.txt" "${common[@]}" --model sonnet ${effort_args[@]+"${effort_args[@]}"} --max-turns 1
    interrupt_signal=""
    run_interrupt "$scratch/interrupt" "$prompts/interrupt.txt" "${common[@]}" --model haiku --max-turns 1

    {
        printf 'recorded_on=%s\n' "$(date +%Y-%m-%d)"
        printf 'credential=%s\n' "$credential"
        printf 'api_key_passed=%s\n' "$api_key_passed"
        printf 'home=%s\n' "$HOME"
        printf 'user=%s\n' "${USER:-$(id -un 2>/dev/null || true)}"
        printf 'host=%s\n' "$(hostname -s 2>/dev/null || true)"
        printf 'scratch=%s\n' "$scratch"
        printf 'scratch_physical=%s\n' "$scratch_physical"
        printf 'stage_marker=%s\n' "$stage_marker"
        printf 'note_marker=%s\n' "$note_marker"
        printf 'auth_listed=%s\n' "$auth_listed"
        printf 'stream_flags=%s\n' "$stream_flags"
        printf 'append_flag=%s\n' "--append-system-prompt"
        printf 'model_flag=%s\n' "--model"
        printf 'effort_mode=%s\n' "$effort_mode"
        printf 'effort_flag=%s\n' "$effort_flag"
        printf 'max_turns_flag=%s\n' "--max-turns"
        printf 'allowed_flag=%s\n' "$allowed_flag"
        printf 'restrict_flag=%s\n' "$restrict_flag"
        printf 'permission_flag=%s\n' "$permission_flag"
        printf 'resume_flag=%s\n' "--resume"
        printf 'settings_flag=%s\n' "$settings_flag"
        printf 'mcp_flag=%s\n' "$mcp_flag"
        printf 'instruction_files=%s\n' "$instruction_files"
        printf 'workdir_in_git=%s\n' "$workdir_in_git"
        printf 'interrupt_signal=%s\n' "$interrupt_signal"
        printf 'env_pass=%s\n' "${MSW_HARNESS_ENV_PASS:-}"
    } >"$raw/run.txt"

    run_derive "$raw" "$out"
}

cmd_derive() {
    [ "$#" -ge 1 ] || usage
    local raw_in="$1" out="$script_dir"
    shift
    if [ "$#" -gt 0 ]; then
        [ "$#" -eq 2 ] && [ "$1" = --out ] || usage
        out="$2"
    fi
    require_python
    [ -f "$raw_in/run.txt" ] || die "$raw_in is not a raw recording (no run.txt)"
    run_derive "$raw_in" "$out"
}

write_fake() {
    cat >"$1" <<'FAKE'
#!/usr/bin/env bash
# Fake Claude Code CLI written by record.sh self-test.
set -u
emit() { printf '%s\n' "$1"; }
uuid() {
    printf '%08x-%04x-4%03x-8%03x-%04x%08x' $((RANDOM * 32768 + RANDOM)) $((RANDOM % 65536)) \
        $((RANDOM % 4096)) $((RANDOM % 4096)) $((RANDOM % 65536)) $((RANDOM * 32768 + RANDOM))
}
case "${1:-}" in
    --version)
        emit '9.9.9 (Claude Code)'
        exit 0
        ;;
    --help)
        cat <<'HELP'
Usage: claude [options] [command] [prompt]

Options:
  -p, --print                       Print response and exit
  --output-format <format>          "text", "json" or "stream-json"
  --verbose                         Verbose output
  --append-system-prompt <prompt>   Append a system prompt
  --model <model>                   Model for the session
  --effort <level>                  Effort level
  --max-turns <turns>               Maximum agentic turns
  --tools <tools...>                Available tools
  --allowedTools, --allowed-tools <tools...>  Tools allowed without asking
  --permission-mode <mode>          Permission mode
  -r, --resume [sessionId]          Resume a conversation
HELP
        # FAKE_NO_ISOLATION=1 plays a CLI without the isolation flags.
        if [ "${FAKE_NO_ISOLATION:-}" != 1 ]; then
            cat <<'HELP'
  --setting-sources <sources>       Setting sources to load
  --strict-mcp-config               Only use MCP servers from --mcp-config
  --mcp-config <configs...>         MCP server configuration
HELP
        fi
        printf '\nCommands:\n  auth                              Manage authentication\n'
        exit 0
        ;;
    auth)
        if [ "${2:-}" = status ] && [ "${3:-}" = --json ]; then
            emit '{"loggedIn":true,"authMethod":"claude.ai","email":"someone@example.com","orgId":"8a3c1f00-1234-4abc-9def-0123456789ab","orgName":"Acme Widgets Ltd","subscriptionType":"max"}'
            exit 0
        fi
        if [ "${2:-}" = status ]; then
            printf 'Logged in as someone@example.com\nOrganization: Acme Widgets Ltd\nLogin method: Claude Max account\n'
            exit 0
        fi
        exit 1
        ;;
esac
model="" resume="" system="" tools="" sources="user,project,local" strict=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --model) model="$2"; shift ;;
        --setting-sources) sources="$2"; shift ;;
        --strict-mcp-config) strict=1 ;;
        --resume) resume="$2"; shift ;;
        --append-system-prompt) system="$2"; shift ;;
        --tools) tools="$2"; shift ;;
    esac
    shift
done
prompt="$(cat)"
scenario="$(printf '%s\n' "$prompt" | sed -n 's/^msw-scenario: //p' | head -n 1)"
printf 'fake claude: %s from %s\n' "$scenario" "$HOME" >&2
sid="${resume:-$(uuid)}"
# Without isolation the operator's MCP servers and plugins show up by name.
tool_list='["Bash","Edit","Read","mcp__acme-private-mcp__search"]'
[ -n "$tools" ] && tool_list="[\"$tools\"]"
mcp_servers='[{"name":"acme-private-mcp","status":"connected"}]'
[ -n "$strict" ] && mcp_servers='[]'
plugins="[{\"name\":\"acme-private-plugin\",\"path\":\"$HOME/.claude/plugins/acme-private-plugin\"}]"
case ",$sources," in *,user,*) ;; *) plugins='[]' ;; esac
emit "{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"$PWD\",\"session_id\":\"$sid\",\"tools\":$tool_list,\"mcp_servers\":$mcp_servers,\"plugins\":$plugins,\"model\":\"claude-$model-fake\",\"permissionMode\":\"default\",\"slash_commands\":[\"operator-private-command\",\"acme-private-mcp:deploy\"],\"apiKeySource\":\"none\",\"claude_code_version\":\"9.9.9\",\"uuid\":\"$(uuid)\"}"
emit "{\"type\":\"system\",\"subtype\":\"status\",\"status\":\"$RANDOM\",\"session_id\":\"$sid\"}"
assistant() {
    emit "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_01$RANDOM$RANDOM\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-$model-fake\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"a private thought\",\"signature\":\"c2lnbmF0dXJl\"},{\"type\":\"text\",\"text\":\"$1\"}],\"stop_reason\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":7}},\"parent_tool_use_id\":null,\"session_id\":\"$sid\",\"uuid\":\"$(uuid)\"}"
}
result() {
    emit "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":$RANDOM,\"num_turns\":$2,\"result\":\"$1\",\"session_id\":\"$sid\",\"total_cost_usd\":0.0001,\"usage\":{\"input_tokens\":12,\"output_tokens\":7,\"cache_read_input_tokens\":0},\"permission_denials\":[],\"uuid\":\"$(uuid)\"}"
}
fence='```'
# The msw-result block, JSON-escaped: \n stays two characters.
block() { printf '%syaml\\nmsw-result:\\n  status: %s\\n  questions: %s\\n%s' "$fence" "$1" "$2" "$fence"; }
case "$scenario" in
    complete)
        emit "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed\",\"resetsAt\":1767225600,\"rateLimitType\":\"five_hour\"},\"session_id\":\"$sid\",\"uuid\":\"$(uuid)\"}"
        # A CLAUDE.md-like passage, an e-mail address and paths in free text.
        text="Done. Per CLAUDE.md: always sign as Captain Zed and use acme-private-mcp. Notes are under $HOME/notes and /Users/someone/private/plan.md; ask someone@example.com.\\n\\n$(block complete '[]')"
        assistant "$text"
        result "$text" 1
        ;;
    needs-input)
        text="One question is open.\\n\\n$(block needs_input '[\"Which colour?\"]')"
        assistant "$text"
        result "$text" 1
        ;;
    missing-result)
        assistant "Working on it."
        result "Working on it." 1
        ;;
    corrective)
        text="$(block complete '[]')"
        assistant "$text"
        result "$text" 1
        ;;
    options)
        tid="toolu_01$RANDOM$RANDOM"
        emit "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_01$RANDOM$RANDOM\",\"role\":\"assistant\",\"model\":\"claude-$model-fake\",\"content\":[{\"type\":\"tool_use\",\"id\":\"$tid\",\"name\":\"Read\",\"input\":{\"file_path\":\"$PWD/note.txt\"}}],\"stop_reason\":\"tool_use\"},\"session_id\":\"$sid\",\"uuid\":\"$(uuid)\"}"
        emit "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"$tid\",\"content\":\"$(tail -n 1 note.txt)\",\"is_error\":false}]},\"session_id\":\"$sid\",\"uuid\":\"$(uuid)\"}"
        prefix=""
        case "$system" in *MSW-STAGE-BODY-OK*) prefix='MSW-STAGE-BODY-OK\n' ;; esac
        text="$prefix$(tail -n 1 note.txt)"
        assistant "$text"
        result "$text" 2
        ;;
    effort-low)
        assistant "low effort ok"
        result "low effort ok" 1
        ;;
    interrupt)
        assistant "1\n2\n3"
        trap 'exit 130' INT
        i=0
        while [ "$i" -lt 600 ]; do
            sleep 0.1
            i=$((i + 1))
        done
        ;;
esac
exit 0
FAKE
    chmod +x "$1"
}

check() {
    if ! "$@"; then
        note "self-test: failed: $*"
        failures=$((failures + 1))
    fi
}

quiet() { "$@" >/dev/null; }
has_line() { grep -Eq -- "$1" "$2"; }
lacks() { ! grep -rqF -- "$1" "$2"; }
first_session() { grep -o '"session_id": "[^"]*"' "$1" | head -n 1; }

cmd_self_test() {
    require_python
    local t run s key status log
    t="$(temp_dir)" || die "mktemp failed"
    cleanup_paths+=("$t")
    failures=0
    mkdir -p "$t/bin"
    write_fake "$t/bin/claude"
    log="$t/record.log"
    for run in 1 2; do
        status=0
        CLAUDE_BIN="$t/bin/claude" MSW_HARNESS_CREDENTIAL=subscription MSW_HARNESS_ENV_PASS="" \
            bash "$script_path" record --raw "$t/raw$run" --out "$t/out$run" >"$log" 2>&1 || status=$?
        # Only the checks on the machine's own directories may fail here: a
        # CLAUDE.md above TMPDIR, or a TMPDIR inside a git work tree.
        if [ "$status" -eq 1 ] && grep -Eq '^record\.sh: not confirmed: (isolation_settings|isolation_workdir)(, isolation_(settings|workdir))?$' "$log" &&
            ! grep -q 'leak' "$log"; then
            note "self-test: run $run: $(grep 'not confirmed' "$log") (from this machine's TMPDIR)"
            status=0
        fi
        if [ "$status" -ne 0 ]; then
            cat "$log" >&2
            note "self-test: failed: record run $run exited $status"
            exit 1
        fi
    done
    local out="$t/out1"
    for s in "${scenarios[@]}"; do
        check test -s "$out/fixtures/$s.jsonl"
        check test -s "$t/raw1/$s.stderr"
    done
    check test ! -e "$out/fixtures/stderr.txt"
    check has_line '^== claude --version ==$' "$out/fixtures/probe.txt"
    check has_line '^9\.9\.9 \(Claude Code\)$' "$out/fixtures/probe.txt"
    check has_line '^== claude auth status --json ==$' "$out/fixtures/probe.txt"
    check has_line '"loggedIn": true' "$out/fixtures/probe.txt"
    check has_line '"authMethod": "claude\.ai"' "$out/fixtures/probe.txt"
    check has_line '"email": "<text:19>"' "$out/fixtures/probe.txt"
    check has_line '^Organization: <text:16>$' "$out/fixtures/probe.txt"
    check has_line '^<text:32>$' "$out/fixtures/probe.txt"
    check has_line 'credential mode .*: subscription$' "$out/fixtures/probe.txt"
    check has_line '^\[claude\."9\.9\.9"\]$' "$out/versions.toml"
    for key in stream_json append_system_prompt model effort max_turns allowed_tools resume isolation_mcp isolation_plugins; do
        check has_line "^$key = \\{ flag = \"[^\"]+\", confirmed = true, evidence = \".+\" \\}$" "$out/versions.toml"
    done
    for key in isolation_settings isolation_workdir; do
        check has_line "^$key = \\{ flag = \"[^\"]*\", confirmed = (true|false), evidence = \".+\" \\}$" "$out/versions.toml"
    done
    check has_line '^rate_limit_event = \["fixtures/complete\.jsonl"\]$' "$out/versions.toml"
    check has_line '^interrupt = \{ signal = "SIGINT", exit_status = 130, result_event = false \}$' "$out/versions.toml"
    # Content-free: free text becomes its length, the init event keeps counts.
    check has_line '"type": "text", "text": "<text:[0-9]+>"' "$out/fixtures/complete.jsonl"
    check has_line '"thinking": "<text:17>"' "$out/fixtures/complete.jsonl"
    check has_line '"result": "<text:[0-9]+>"' "$out/fixtures/complete.jsonl"
    check has_line '"rateLimitType": "five_hour"' "$out/fixtures/complete.jsonl"
    check has_line '"resetsAt": 0' "$out/fixtures/complete.jsonl"
    check has_line '"subtype": "init".*"tools": 4, "mcp_servers": 0, "plugins": 0,.*"slash_commands": 2' "$out/fixtures/complete.jsonl"
    check has_line '"tools": 1,' "$out/fixtures/options.jsonl"
    check has_line '"name": "Read", "input": \{"file_path": "<text:[0-9]+>"\}' "$out/fixtures/options.jsonl"
    check has_line '"model": "claude-haiku-fake"' "$out/fixtures/complete.jsonl"
    check has_line '"session_id": "<id:[0-9]+>"' "$out/fixtures/complete.jsonl"
    local private=(acme-private-mcp acme-private-plugin operator-private-command "Captain Zed" "Per CLAUDE" Acme
        someone@example.com /Users/someone "a private thought" "MSW harness note" "$note_marker" "$stage_marker"
        "Working on it" mcp__ "$HOME" "$t")
    for s in "${private[@]}"; do
        check lacks "$s" "$out"
    done
    check test "$(first_session "$out/fixtures/corrective.jsonl")" = "$(first_session "$out/fixtures/missing-result.jsonl")"
    check test -n "$(first_session "$out/fixtures/corrective.jsonl")"

    check quiet py compare "$t/out1" "$t/out2"
    cp -R "$t/out2" "$t/out3"
    printf '%s\n' '{"type": "user"}' >>"$t/out3/fixtures/complete.jsonl"
    status=0
    py compare "$t/out1" "$t/out3" >/dev/null || status=$?
    check test "$status" -eq 1

    check py leak-check "$out"
    mkdir -p "$t/leaky"
    printf '%s\n' '{"text": "mail someone@example.com"}' >"$t/leaky/x.jsonl"
    status=0
    py leak-check "$t/leaky" >/dev/null 2>&1 || status=$?
    check test "$status" -eq 1

    # A value off the allowlist stops the write: free text, a non-enum under
    # an enum field, a field name that is not an identifier, an unknown line
    # in probe.txt's place.
    local inject
    for inject in '{"type": "assistant", "note": "plain words"}' '{"type": "Captain Zed"}' \
        '{"type": "assistant", "acme-private-mcp": 1}' '{"type": "assistant", "model": "/Users/someone/model"}'; do
        status=0
        MSW_HARNESS_INJECT="$inject" bash "$script_path" derive "$t/raw1" --out "$t/out5" >/dev/null 2>&1 || status=$?
        check test "$status" -eq 1
        check test ! -e "$t/out5"
    done

    # A CLI without the isolation flags: recorded as not confirmed, and the
    # operator's MCP server and plugin names still stay out of the fixtures.
    status=0
    CLAUDE_BIN="$t/bin/claude" MSW_HARNESS_ENV_PASS=FAKE_NO_ISOLATION FAKE_NO_ISOLATION=1 \
        bash "$script_path" record --raw "$t/raw6" --out "$t/out6" >"$log" 2>&1 || status=$?
    check test "$status" -eq 1
    for key in isolation_settings isolation_mcp isolation_plugins; do
        check has_line "^$key = \\{ flag = \"[^\"]+\", confirmed = false, evidence = \".+\" \\}$" "$t/out6/versions.toml"
    done
    check has_line '^isolation_mcp = .*MCP servers reported by system/init: the highest count is 1 over 7 scenarios' "$t/out6/versions.toml"
    check has_line '"mcp_servers": 1, "plugins": 1' "$t/out6/fixtures/complete.jsonl"
    for s in "${private[@]}"; do
        check lacks "$s" "$t/out6"
    done
    check py leak-check "$t/out6"

    local root
    root="$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || true)"
    if [ -n "$root" ]; then
        status=0
        CLAUDE_BIN="$t/bin/claude" bash "$script_path" record --raw "$script_dir/raw-refused" --out "$t/out4" >/dev/null 2>&1 || status=$?
        check test "$status" -eq 2
        check test ! -e "$script_dir/raw-refused"
    fi

    if [ "$failures" -ne 0 ]; then
        note "self-test: $failures check(s) failed"
        exit 1
    fi
    printf '%s\n' "record.sh self-test: ok"
}

[ "$#" -ge 1 ] || usage
mode="$1"
shift
case "$mode" in
    record) cmd_record "$@" ;;
    derive) cmd_derive "$@" ;;
    compare)
        [ "$#" -eq 2 ] || usage
        require_python
        py compare "$1" "$2"
        ;;
    leak-check)
        [ "$#" -eq 1 ] || usage
        require_python
        py leak-check "$1"
        ;;
    self-test)
        [ "$#" -eq 0 ] || usage
        cmd_self_test
        ;;
    *) usage ;;
esac
