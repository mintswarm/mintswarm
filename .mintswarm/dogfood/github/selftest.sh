#!/usr/bin/env bash
# Offline self-test of the mintswarm-dev tooling in this directory.
#
#   selftest.sh
#
# Runs in a fresh mktemp -d sandbox with HOME, TMPDIR, XDG_RUNTIME_DIR and
# MSW_APP_CONFIG inside it, and PATH starting with stubs for security, secret-tool,
# pass, systemd-creds, uname, curl and gh that record their argv and stdin. It
# aborts before any check unless those commands resolve to the stubs, so no real
# key store and not GitHub are ever reached. The key store settings
# (MSW_KEY_STORE, DBUS_SESSION_BUS_ADDRESS, PASSWORD_STORE_DIR, GNUPGHOME,
# CREDENTIALS_DIRECTORY) are unset. uname -s answers $MSW_STUB_UNAME, Darwin by
# default. git runs without the system config, against repositories and a bare
# "remote" inside the sandbox.
#
# Checks: shellcheck and py_compile; the per-App table; the manifests; App
# creation with store-credentials.sh create, whose listener is driven over
# 127.0.0.1 (matching and wrong states, loopback only, the timeout, a client that
# trickles bytes, a held port) with the conversion answered by the curl stub;
# the JWT and the installation token; key rotation for both Apps, with no
# per-use confirmation; every key store backend (selection, round trips, put
# --confirm, the file backend's modes and refusals, rotate and jwt through
# each); the bot identity include; push.sh pushes and refusals; the inline
# credential helper; bot-gh.sh; reviewer resolution; assignment and release of
# approvers; re-classification of a split issue and the parent check before
# children are filed; landing a PR as the steward with land.py, and each of its
# refusals; msw-state.py in field and labels mode (exact bodies, no-op re-runs,
# a foreign field id, unavailable fields, ignored writes, a single label swapped
# in one PUT, label names in any case) and the issue status it maps to (reset
# after GitHub moves it, none and status: off leave it alone), and the README's
# msw-state tables against the script's VALUES and STATUS. Secrets and the
# manifest code must appear in no recorded argv, nothing may be left in TMPDIR,
# each create adds only its own App's config to HOME, and .git/config stays
# byte-identical across pushes. Every background process runs in its own
# process group, every wait is bounded, and any group still there is killed on
# exit.
#
# Prints one line per check and exits 0 only if every check passed.
# Needs shellcheck, jq, yq, openssl, python3 and git.
#
# shellcheck disable=SC2016,SC2329,SC2030,SC2031,SC2012
# SC2016: sh -c '...' scripts take their values as positional parameters.
# SC2012: ls -ld and ls -A list sandbox paths the test created, for their modes and names.
# SC2329: check functions are invoked through check and refuses.
# SC2030, SC2031: subshells that source common.sh change variables on purpose.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sandbox="$(mktemp -d "${TMPDIR:-/tmp}/msw-selftest.XXXXXX")"
sandbox="$(cd "$sandbox" && pwd -P)"
bg_groups=""   # process groups started in the background, killed on exit if still there
cleanup() {
  local g
  for g in $bg_groups; do kill -TERM -- "-$g" 2>/dev/null || true; done
  rm -rf "$sandbox"
}
trap cleanup EXIT
trap 'printf "selftest: setup failed at line %s\n" "$LINENO" >&2' ERR

unset MSW_TOKEN MSW_PUSH_URL GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN \
      GH_REPO GH_HOST GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR \
      GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_AUTHOR_DATE GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL \
      GIT_COMMITTER_DATE GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GIT_CONFIG_GLOBAL GIT_CONFIG \
      GIT_ASKPASS SSH_ASKPASS GIT_SSL_NO_VERIFY GIT_SSL_CAINFO GIT_SSL_CAPATH \
      MSW_KEY_STORE DBUS_SESSION_BUS_ADDRESS PASSWORD_STORE_DIR GNUPGHOME CREDENTIALS_DIRECTORY \
      MSW_STUB_UNAME MSW_STUB_SYSTEMD_VERSION MSW_STUB_SECRET_TOOL_HANG MSW_KEY_STORE_TIMEOUT \
      MSW_STEWARD_CONFIG MSW_MANIFEST_TIMEOUT MSW_STUB_CONV_SLUG MSW_STUB_CONV_MANIFEST MSW_STUB_CURL_FAIL \
      MSW_STUB_SECURITY_READ_FAIL MSW_STUB_SECURITY_WRITE_FAIL MSW_LAND_RETRY_DELAY
export HOME="$sandbox/home"
export TMPDIR="$sandbox/tmp"
export XDG_RUNTIME_DIR="$sandbox/run"
export MSW_APP_CONFIG="$HOME/.config/mintswarm/dev-app.json"
export MSW_STUB_STATE="$sandbox/stub-state"
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
stubs="$sandbox/stubs"
keys="$sandbox/keys"
mkdir -p "$HOME" "$TMPDIR" "$XDG_RUNTIME_DIR" "$MSW_STUB_STATE" "$stubs" "$keys"

DUMMY=ghs_SelfTestDummyToken0123456789abcdefABCDEF
MINTED=ghs_SelfTestMintedToken9876543210fedcbaFEDCBA
CLIENT_ID=Iv23liSelfTest0001
CODE=selftestcode
BOT="mintswarm-dev[bot]|359856778+mintswarm-dev[bot]@users.noreply.github.com"
BOT_IDENT="$BOT|$BOT"
MAINT="Maintainer|maintainer@example.com"
export MSW_STUB_MINTED="$MINTED" MSW_STUB_CLIENT_ID="$CLIENT_ID" MSW_STUB_CODE="$CODE"

# --- stubs -------------------------------------------------------------------

cat > "$stubs/stublib.py" <<'EOF'
import json, os, sys

STATE = os.environ["MSW_STUB_STATE"]


def record(cmd, argv):
    with open(os.path.join(STATE, "argv.log"), "a") as f:
        f.write(json.dumps({"cmd": cmd, "argv": argv}) + "\n")


def record_stdin(cmd, data):
    with open(os.path.join(STATE, "stdin-" + cmd + ".log"), "a") as f:
        f.write(data + "\n--\n")


def opt(argv, flag):
    for i, a in enumerate(argv[:-1]):
        if a == flag:
            return argv[i + 1]
    return None
EOF

cat > "$stubs/security" <<'EOF'
#!/usr/bin/env python3
# Stub of macOS security: a keychain kept in a JSON file in the stub state.
import json, os, shlex, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin, opt

argv = sys.argv[1:]
record("security", argv)
path = os.path.join(STATE, "keychain.json")
kc = json.load(open(path)) if os.path.exists(path) else {}

if argv == ["-i"]:
    data = sys.stdin.read()
    record_stdin("security", data)
    if os.environ.get("MSW_STUB_SECURITY_WRITE_FAIL"):
        # As when the keychain refuses the write.
        print("security: SecKeychainItemCreateFromContent: Write permissions error.", file=sys.stderr)
        sys.exit(1)
    for line in data.splitlines():
        words = shlex.split(line)
        if not words:
            continue
        if words[0] != "add-generic-password":
            sys.exit(1)
        s, a, w = opt(words, "-s"), opt(words, "-a"), opt(words, "-w")
        if not (s and a and w):
            sys.exit(1)
        if "-U" not in words and s + "/" + a in kc:
            # As the real one: without -U an existing item is not replaced.
            print("security: The specified item already exists in the keychain.", file=sys.stderr)
            sys.exit(45)
        kc[s + "/" + a] = w
    json.dump(kc, open(path, "w"))
    sys.exit(0)

if argv and argv[0] in ("find-generic-password", "delete-generic-password"):
    key = (opt(argv, "-s") or "") + "/" + (opt(argv, "-a") or "")
    if key not in kc:
        print("security: The specified item could not be found in the keychain.", file=sys.stderr)
        sys.exit(44)
    if argv[0] == "delete-generic-password":
        del kc[key]
        json.dump(kc, open(path, "w"))
        sys.exit(0)
    if "-w" in argv and os.environ.get("MSW_STUB_SECURITY_READ_FAIL"):
        # As when the item cannot be read, for example from a locked keychain.
        print("security: User interaction is not allowed.", file=sys.stderr)
        sys.exit(36)
    print(kc[key] if "-w" in argv else 'keychain: "stub"')
    sys.exit(0)

sys.exit(1)
EOF

cat > "$stubs/secret-tool" <<'EOF'
#!/usr/bin/env python3
# Stub of secret-tool (libsecret): store, lookup and clear, kept in a JSON file.
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin

argv = sys.argv[1:]
record("secret-tool", argv)
if os.environ.get("MSW_STUB_SECRET_TOOL_HANG"):
    # A locked collection: secret-tool waits for an unlock prompt nobody answers.
    import time
    with open(os.path.join(STATE, "secret-tool.pid"), "w") as f:
        f.write("%d\n" % os.getpid())
    time.sleep(60)
    sys.exit(0)
if not os.environ.get("DBUS_SESSION_BUS_ADDRESS"):
    print("secret-tool: Cannot autolaunch D-Bus without X11 $DISPLAY", file=sys.stderr)
    sys.exit(1)
path = os.path.join(STATE, "secret-service.json")
db = json.load(open(path)) if os.path.exists(path) else {}
cmd, rest = (argv[0], argv[1:]) if argv else ("", [])
labels = [a for a in rest if a.startswith("--label=")]
attrs = [a for a in rest if not a.startswith("--")]
if len(attrs) % 2 or not attrs:
    sys.exit(2)
key = "/".join(attrs)
if cmd == "store" and labels:
    data = sys.stdin.read()
    record_stdin("secret-tool", data)
    db[key] = data
    json.dump(db, open(path, "w"))
elif cmd == "lookup":
    if key not in db:
        sys.exit(1)
    sys.stdout.write(db[key])
elif cmd == "clear":
    db.pop(key, None)
    json.dump(db, open(path, "w"))
else:
    sys.exit(2)
EOF

cat > "$stubs/pass" <<'EOF'
#!/usr/bin/env python3
# Stub of pass: insert -m -f, show and rm -f, kept in a JSON file.
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin

argv = sys.argv[1:]
record("pass", argv)
store = os.environ.get("PASSWORD_STORE_DIR") or os.path.join(os.environ["HOME"], ".password-store")
if not os.path.exists(os.path.join(store, ".gpg-id")):
    print("Error: You must run: pass init your-gpg-id", file=sys.stderr)
    sys.exit(1)
path = os.path.join(STATE, "pass.json")
db = json.load(open(path)) if os.path.exists(path) else {}
if argv[:3] == ["insert", "-m", "-f"] and len(argv) == 4:
    print("Enter contents of %s and press Ctrl+D when finished:" % argv[3])
    data = sys.stdin.read()
    record_stdin("pass", data)
    db[argv[3]] = data
elif argv[:1] == ["show"] and len(argv) == 2:
    if argv[1] not in db:
        print("Error: %s is not in the password store." % argv[1], file=sys.stderr)
        sys.exit(1)
    sys.stdout.write(db[argv[1]])
elif argv[:2] == ["rm", "-f"] and len(argv) == 3:
    if argv[2] not in db:
        print("Error: %s is not in the password store." % argv[2], file=sys.stderr)
        sys.exit(1)
    del db[argv[2]]
else:
    sys.exit(1)
json.dump(db, open(path, "w"))
EOF

cat > "$stubs/systemd-creds" <<'EOF'
#!/usr/bin/env python3
# Stub of systemd-creds: --version, and encrypt and decrypt with --name and
# stdin or stdout. The "encrypted" file is a marker line and the reversed value
# in base64, so it holds no plaintext line.
import base64, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin

argv = sys.argv[1:]
record("systemd-creds", argv)
if argv == ["--version"]:
    print("systemd %s (stub)" % os.environ.get("MSW_STUB_SYSTEMD_VERSION", "257"))
    sys.exit(0)
args = [a for a in argv if a != "--user"]
names = [a[len("--name="):] for a in args if a.startswith("--name=")]
args = [a for a in args if not a.startswith("--name=")]
if len(names) != 1 or len(args) != 3:
    sys.exit(2)
marker = "SYSTEMD-CREDS-STUB name=%s user=%s" % (names[0], "--user" in argv)
if args[0] == "encrypt" and args[1] == "-":
    data = sys.stdin.read()
    record_stdin("systemd-creds", data)
    with open(args[2], "w") as f:
        f.write(marker + "\n" + base64.b64encode(data[::-1].encode()).decode() + "\n")
elif args[0] == "decrypt" and args[2] == "-":
    with open(args[1]) as f:
        head, body = f.read().split("\n", 1)
    if head != marker:
        print("systemd-creds: Embedded credential name or scope does not match", file=sys.stderr)
        sys.exit(1)
    sys.stdout.write(base64.b64decode(body.strip()).decode()[::-1])
else:
    sys.exit(2)
EOF

cat > "$stubs/uname" <<'EOF'
#!/usr/bin/env python3
# Stub of uname: -s answers $MSW_STUB_UNAME (Darwin by default); anything else fails.
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import record

record("uname", sys.argv[1:])
if sys.argv[1:] != ["-s"]:
    sys.exit(1)
print(os.environ.get("MSW_STUB_UNAME", "Darwin"))
EOF

cat > "$sandbox/on-tty.py" <<'EOF'
# on-tty.py COMMAND...: run COMMAND with stdout on a pseudo-terminal; print what
# reached the terminal on stdout and exit with COMMAND's status.
import os, subprocess, sys

m, s = os.openpty()
p = subprocess.Popen(sys.argv[1:], stdout=s, stderr=subprocess.PIPE)
os.close(s)
out = b""
while True:
    try:
        chunk = os.read(m, 4096)
    except OSError:
        break
    if not chunk:
        break
    out += chunk
err = p.stderr.read()
rc = p.wait()
sys.stdout.write(out.decode(errors="replace"))
sys.stderr.write(err.decode(errors="replace"))
sys.exit(rc)
EOF

cat > "$stubs/curl" <<'EOF'
#!/usr/bin/env python3
# Stub of curl for the GitHub endpoints mint-token.sh and store-credentials.sh call.
# Headers come on stdin with -H @-; with -K - the stdin is a config whose url line
# gives the URL. The conversion answers slug $MSW_STUB_CONV_SLUG (mintswarm-dev
# by default), owner mintswarm, and the permissions and events of the manifest
# named by $MSW_STUB_CONV_MANIFEST, when set.
import json, os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin, opt

argv = sys.argv[1:]
record("curl", argv)
headers = ""
config = ""
if any(argv[i] == "-H" and argv[i + 1] == "@-" for i in range(len(argv) - 1)):
    headers = sys.stdin.read()
    record_stdin("curl", headers)
if any(argv[i] == "-K" and argv[i + 1] == "-" for i in range(len(argv) - 1)):
    config = sys.stdin.read()
    record_stdin("curl", config)
if os.environ.get("MSW_STUB_CURL_FAIL"):
    sys.exit(22)
urls = [a for a in argv if a.startswith("https://")]
m = re.search(r'^url = "(https://[^"]*)"$', config, re.M)
if m:
    urls.append(m.group(1))
url = urls[-1] if urls else ""
authed = re.search(r"^Authorization: Bearer [A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$", headers, re.M)

if url == "https://api.github.com/repos/mintswarm/mintswarm/installation" and authed:
    print(json.dumps({"id": 4242}))
elif url == "https://api.github.com/app/installations/4242/access_tokens" and authed:
    print(json.dumps({"token": os.environ["MSW_STUB_MINTED"], "expires_at": "2026-11-22T13:00:00Z"}))
elif url == "https://api.github.com/app-manifests/%s/conversions" % os.environ["MSW_STUB_CODE"] \
        and opt(argv, "-X") == "POST":
    pem = open(os.path.join(STATE, "conv-key.pem")).read()
    conv = {"id": 5874301, "slug": os.environ.get("MSW_STUB_CONV_SLUG", "mintswarm-dev"),
            "owner": {"login": "mintswarm"}, "client_id": os.environ["MSW_STUB_CLIENT_ID"],
            "client_secret": "selftest-client-secret-value", "webhook_secret": "selftest-webhook-secret",
            "pem": pem}
    if os.environ.get("MSW_STUB_CONV_MANIFEST"):
        manifest = json.load(open(os.environ["MSW_STUB_CONV_MANIFEST"]))
        conv["permissions"] = manifest["default_permissions"]
        conv["events"] = manifest["default_events"]
    print(json.dumps(conv))
else:
    print("curl: (22) The requested URL returned error: 404", file=sys.stderr)
    sys.exit(22)
EOF

cat > "$stubs/gh" <<'EOF'
#!/usr/bin/env python3
# Stub of gh: records argv, the environment it was given and stdin for --input -.
# Issues, their assignees, events, type and comments are kept in issues.json in
# the stub state: {"<n>": {"assignees": [...], "events": [...], "unassignable":
# [...], "type": "<name>" or null, "comments": [...], "untypeable": bool,
# "comment_failures": n, "comment_reads": [[...], ...]}}; a PATCH of the issue
# sets its type unless it is untypeable; a comment POST fails while
# comment_failures is above 0, counting it down; a GET of the comments serves
# one list of comment_reads per read, the last repeating, when it is set,
# counted in comments-reads.json; a PR's reviews and inline review comments,
# served for pulls/<n>/reviews and pulls/<n>/comments, in reviews.json and
# review-comments.json: {"<n>": [...]} or {"<n>": {"reads": [[...], ...]}},
# one list per read, the last repeating, counted in reviews-reads.json and
# review-comments-reads.json.
# For land.py: pulls/<n> from pulls.json, {"<n>": [read 1, read 2, ...]}, the
# last read repeating, counted in pulls-reads.json; check runs, check suites
# and statuses per commit in commits.json, {"<sha>": {"check_runs": [...],
# "check_suites": [...], "statuses": [...], "status_total": n}} or
# {"<sha>": {"reads": [{...}, ...]}}, one per read of each of check-runs,
# check-suites and status, the last repeating, counted per "<sha>/<kind>" in
# commits-reads.json, check runs and suites two to a page; review threads for
# "api graphql" with no body on stdin in threads.json,
# {"<n>": [isResolved, ...]} or {"<n>": {"reads": [[isResolved, ...], ...]}},
# one list per read of the first page, the last repeating, counted in
# threads-reads.json, two to a page with the index as cursor; and PUT
# pulls/<n>/merge, recorded in merges.json and answered 405 while merges.json
# has "merge_refused": true, after which pulls/<n> answers merged, by
# mintswarm-dev-steward[bot].
# For msw-state.py an issue also keeps "fields" (its issue-field values, served
# and POSTed at issues/<n>/issue-field-values; a POST replaces the value of each
# field id given, named msw-state, unless "field_ignores_post"; with
# "fields_unavailable" both answer HTTP 404), "labels" (names, served and PUT at
# issues/<n>/labels) and "status" (a status name or null; the issue GET returns
# it with node_id I_<n>). The repository's labels are in labels.json (served and
# POSTed at repos/.../labels) and its issue statuses in statuses.json
# ([{"id", "name", "category"}], four by default); api graphql with --input -
# answers the planningStatuses query from them and applies setIssueStatus to the issue whose
# node id is I_<n>, unless that issue has "status_ignores_set".
import json, os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin, opt

STATUSES = [{"id": "S_todo", "name": "Todo", "category": "TODO"},
            {"id": "S_progress", "name": "In progress", "category": "IN_PROGRESS"},
            {"id": "S_review", "name": "In review", "category": "IN_PROGRESS"},
            {"id": "S_done", "name": "Done", "category": "DONE"}]
argv = sys.argv[1:]
record("gh", argv)
names = ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN",
         "GH_REPO", "GH_HOST", "GH_PROMPT_DISABLED", "MSW_TOKEN"]
with open(os.path.join(STATE, "gh-env.log"), "a") as f:
    f.write(json.dumps({n: os.environ.get(n) for n in names}) + "\n")
data = ""
if any(argv[i] == "--input" and argv[i + 1] == "-" for i in range(len(argv) - 1)):
    data = sys.stdin.read()
    record_stdin("gh", data)
if argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/pulls/\d+/(reviews|comments)", a) for a in argv):
    n, kind = re.search(r"/pulls/(\d+)/(reviews|comments)", next(a for a in argv if a.startswith("repos/"))).groups()
    name = "reviews" if kind == "reviews" else "review-comments"
    store = os.path.join(STATE, name + ".json")
    reviews = (json.load(open(store)) if os.path.exists(store) else {}).get(n, [])
    if isinstance(reviews, dict):
        reads_path = os.path.join(STATE, name + "-reads.json")
        reads = json.load(open(reads_path)) if os.path.exists(reads_path) else {}
        seen = reads.get(n, 0)
        reads[n] = seen + 1
        json.dump(reads, open(reads_path, "w"))
        reviews = reviews["reads"][min(seen, len(reviews["reads"]) - 1)]
    print(json.dumps([reviews] if "--slurp" in argv else reviews))
elif argv[:2] == ["api", "graphql"] and not data:
    store = os.path.join(STATE, "threads.json")
    n = (opt(argv, "-F") or "number=0").split("=", 1)[1]
    after = [a.split("=", 1)[1] for i, a in enumerate(argv) if argv[i - 1] == "-f" and a.startswith("after=")]
    start = int(after[0]) if after else 0
    threads = (json.load(open(store)) if os.path.exists(store) else {}).get(n)
    if isinstance(threads, dict):
        reads_path = os.path.join(STATE, "threads-reads.json")
        reads = json.load(open(reads_path)) if os.path.exists(reads_path) else {}
        seen = reads.get(n, 0) - (0 if start == 0 else 1)
        if start == 0:
            reads[n] = seen + 1
            json.dump(reads, open(reads_path, "w"))
        threads = threads["reads"][min(seen, len(threads["reads"]) - 1)]
    if threads is None:
        print(json.dumps({"data": {"repository": {"pullRequest": None}}}))
        sys.exit(0)
    page = threads[start:start + 2]
    more = start + 2 < len(threads)
    print(json.dumps({"data": {"repository": {"pullRequest": {"reviewThreads": {
        "nodes": [{"isResolved": r} for r in page],
        "pageInfo": {"hasNextPage": more, "endCursor": str(start + 2) if more else None}}}}}}))
elif argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/pulls/\d+/merge", a) for a in argv):
    store = os.path.join(STATE, "merges.json")
    merges = json.load(open(store)) if os.path.exists(store) else {}
    n = re.search(r"/pulls/(\d+)/", next(a for a in argv if a.startswith("repos/"))).group(1)
    merges.setdefault("calls", []).append({"pr": n, "method": opt(argv, "--method"), "body": json.loads(data or "null")})
    if merges.get("merge_refused"):
        json.dump(merges, open(store, "w"))
        print("gh: Repository rule violations found (HTTP 405)", file=sys.stderr)
        sys.exit(1)
    merges.setdefault("merged", []).append(n)
    json.dump(merges, open(store, "w"))
    print(json.dumps({"sha": "f" * 40, "merged": True, "message": "Pull Request successfully merged"}))
elif argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/pulls/\d+", a) for a in argv):
    n = re.search(r"/pulls/(\d+)$", next(a for a in argv if a.startswith("repos/"))).group(1)
    reads_path = os.path.join(STATE, "pulls-reads.json")
    reads = json.load(open(reads_path)) if os.path.exists(reads_path) else {}
    seen = reads.get(n, 0)
    reads[n] = seen + 1
    json.dump(reads, open(reads_path, "w"))
    versions = json.load(open(os.path.join(STATE, "pulls.json")))[n]
    pr = dict(versions[min(seen, len(versions) - 1)])
    merges_path = os.path.join(STATE, "merges.json")
    if os.path.exists(merges_path) and n in json.load(open(merges_path)).get("merged", []):
        pr.update({"state": "closed", "merged": True, "merge_commit_sha": "f" * 40,
                   "merged_by": {"login": "mintswarm-dev-steward[bot]"}})
    print(json.dumps(pr))
elif argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/commits/[0-9a-f]+/(check-runs|check-suites|status)(\?.*)?", a) for a in argv):
    sha, kind = re.search(r"/commits/([0-9a-f]+)/(check-runs|check-suites|status)", next(a for a in argv if a.startswith("repos/"))).groups()
    store = os.path.join(STATE, "commits.json")
    commit = (json.load(open(store)) if os.path.exists(store) else {}).get(sha, {})
    if "reads" in commit:
        reads_path = os.path.join(STATE, "commits-reads.json")
        reads = json.load(open(reads_path)) if os.path.exists(reads_path) else {}
        seen = reads.get(sha + "/" + kind, 0)
        reads[sha + "/" + kind] = seen + 1
        json.dump(reads, open(reads_path, "w"))
        commit = commit["reads"][min(seen, len(commit["reads"]) - 1)]
    if kind == "status":
        statuses = commit.get("statuses", [])
        print(json.dumps({"state": "success", "total_count": commit.get("status_total", len(statuses)),
                          "statuses": statuses}))
    else:
        key = kind.replace("-", "_")
        items = commit.get(key, [])
        pages = [{"total_count": len(items), key: items[i:i + 2]} for i in range(0, len(items), 2)]
        pages = pages or [{"total_count": 0, key: []}]
        print(json.dumps(pages if "--slurp" in argv else pages[0]))
elif argv[:1] == ["api"] and any(a.endswith("/requested_reviewers") for a in argv):
    reviewers = json.loads(data)["reviewers"]
    print(json.dumps({"number": 7, "requested_reviewers": [{"login": r} for r in reviewers]}))
elif argv[:2] == ["api", "graphql"]:
    store = os.path.join(STATE, "issues.json")
    issues = json.load(open(store)) if os.path.exists(store) else {}
    spath = os.path.join(STATE, "statuses.json")
    statuses = json.load(open(spath)) if os.path.exists(spath) else STATUSES
    req = json.loads(data)
    if "planningStatuses" in req["query"]:
        print(json.dumps({"data": {"node": {"repository": {"planningStatuses": {"nodes": statuses}}}}}))
    elif "setIssueStatus" in req["query"]:
        n = req["variables"]["issue"][len("I_"):]
        item = issues.setdefault(n, {})
        match = [s for s in statuses if s["id"] == req["variables"]["status"]]
        if not match:
            print(json.dumps({"data": None, "errors": [{"message": "no such status"}]}))
            sys.exit(1)
        if not item.get("status_ignores_set"):
            item["status"] = match[0]["name"]
        json.dump(issues, open(store, "w"))
        print(json.dumps({"data": {"setIssueStatus": {"clientMutationId": None}}}))
    else:
        sys.exit(1)
elif argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/issues/\d+/(issue-field-values|labels)", a) for a in argv):
    path = next(a for a in argv if a.startswith("repos/"))
    n, tail = re.fullmatch(r"repos/[^/]+/[^/]+/issues/(\d+)/(.*)", path).groups()
    method = opt(argv, "--method") or "GET"
    store = os.path.join(STATE, "issues.json")
    issues = json.load(open(store)) if os.path.exists(store) else {}
    item = issues.setdefault(n, {})
    if tail == "issue-field-values":
        if item.get("fields_unavailable"):
            print("gh: Not Found (HTTP 404)", file=sys.stderr)
            sys.exit(1)
        fields = item.setdefault("fields", [])
        if method == "POST" and not item.get("field_ignores_post"):
            for fv in json.loads(data)["issue_field_values"]:
                fields[:] = [f for f in fields if f["issue_field_id"] != fv["field_id"]]
                fields.append({"issue_field_id": fv["field_id"], "node_id": "IFV_%d" % fv["field_id"],
                               "data_type": "single_select", "issue_field_name": "msw-state", "value": fv["value"],
                               "single_select_option": {"id": 1, "name": fv["value"], "color": "GRAY"}})
        out = fields
    else:
        if method == "PUT":
            item["labels"] = list(json.loads(data)["labels"])
        out = [{"name": name} for name in item.setdefault("labels", [])]
        if "--slurp" in argv:
            out = [out]
    json.dump(issues, open(store, "w"))
    print(json.dumps(out))
elif argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/labels", a) for a in argv):
    store = os.path.join(STATE, "labels.json")
    labels = json.load(open(store)) if os.path.exists(store) else []
    if opt(argv, "--method") == "POST":
        label = json.loads(data)
        labels.append(label)
        json.dump(labels, open(store, "w"))
        print(json.dumps(label))
    else:
        print(json.dumps([labels] if "--slurp" in argv else labels))
elif argv[:1] == ["api"] and any(re.fullmatch(r"repos/[^/]+/[^/]+/issues/\d+(/assignees|/events|/comments)?", a) for a in argv):
    path = next(a for a in argv if a.startswith("repos/"))
    n, tail = re.fullmatch(r"repos/[^/]+/[^/]+/issues/(\d+)(.*)", path).groups()
    method = opt(argv, "--method") or "GET"
    store = os.path.join(STATE, "issues.json")
    issues = json.load(open(store)) if os.path.exists(store) else {}
    item = issues.setdefault(n, {})
    for k in ("assignees", "events", "unassignable", "comments"):
        item.setdefault(k, [])
    item.setdefault("type", None)
    bot = {"login": "mintswarm-dev[bot]"}
    app = {"slug": "mintswarm-dev"}
    if tail == "/assignees" and method in ("POST", "DELETE"):
        for login in json.loads(data)["assignees"]:
            if method == "POST" and login not in item["assignees"] and login not in item["unassignable"]:
                item["assignees"].append(login)
                item["events"].append({"event": "assigned", "actor": bot, "assignee": {"login": login},
                                       "assigner": bot, "performed_via_github_app": app})
            elif method == "DELETE" and login in item["assignees"]:
                item["assignees"].remove(login)
                item["events"].append({"event": "unassigned", "actor": bot, "assignee": {"login": login},
                                       "assigner": bot, "performed_via_github_app": app})
        json.dump(issues, open(store, "w"))
    if tail == "" and method == "PATCH":
        if not item.get("untypeable"):
            item["type"] = json.loads(data)["type"]
        json.dump(issues, open(store, "w"))
    if tail == "/comments" and method == "POST" and item.get("comment_failures", 0) > 0:
        item["comment_failures"] -= 1
        json.dump(issues, open(store, "w"))
        print("gh: HTTP 502: Bad Gateway", file=sys.stderr)
        sys.exit(1)
    if tail == "/comments" and method == "POST":
        url = "https://github.com/mintswarm/mintswarm/issues/%s#issuecomment-%d" % (n, 100 + len(item["comments"]))
        comment = {"body": json.loads(data)["body"], "html_url": url}
        item["comments"].append(comment)
        json.dump(issues, open(store, "w"))
        out = comment
    elif tail == "/comments":
        comments = item["comments"]
        if "comment_reads" in item:
            reads_path = os.path.join(STATE, "comments-reads.json")
            reads = json.load(open(reads_path)) if os.path.exists(reads_path) else {}
            seen = reads.get(n, 0)
            reads[n] = seen + 1
            json.dump(reads, open(reads_path, "w"))
            comments = item["comment_reads"][min(seen, len(item["comment_reads"]) - 1)]
        out = [comments] if "--slurp" in argv else comments
    elif tail == "/events":
        out = [item["events"]] if "--slurp" in argv else item["events"]
    else:
        spath = os.path.join(STATE, "statuses.json")
        statuses = json.load(open(spath)) if os.path.exists(spath) else STATUSES
        status = next((s for s in statuses if s["name"] == item.get("status")), None)
        out = {"number": int(n), "node_id": "I_%s" % n, "assignees": [{"login": a} for a in item["assignees"]],
               "type": {"name": item["type"]} if item["type"] else None,
               "status": {"category": status["category"], "id": status["id"], "name": status["name"]}
               if status else None}
    print(json.dumps(out))
else:
    print(json.dumps({"full_name": "mintswarm/mintswarm"}))
EOF
chmod 755 "$stubs/security" "$stubs/secret-tool" "$stubs/pass" "$stubs/systemd-creds" "$stubs/uname" \
          "$stubs/curl" "$stubs/gh"

export PATH="$stubs:$PATH"
for c in security secret-tool pass systemd-creds uname curl gh; do
  if [ "$(command -v "$c")" != "$stubs/$c" ]; then
    printf 'selftest: %s does not resolve to the stub; aborting before anything runs\n' "$c" >&2
    exit 1
  fi
done

# --- helpers -----------------------------------------------------------------

passed=0
failed=0
ok() { passed=$((passed + 1)); printf 'ok    %s\n' "$1"; }
not_ok() { failed=$((failed + 1)); printf 'FAIL  %s\n' "$1"; }
check() {  # check NAME COMMAND...: COMMAND succeeds; its output is shown only on failure
  local name="$1"; shift
  if "$@" >"$sandbox/check.out" 2>&1; then
    ok "$name"
  else
    not_ok "$name"; sed 's/^/      /' "$sandbox/check.out"
  fi
}
refuses() {  # refuses NAME COMMAND...: COMMAND fails
  local name="$1"; shift
  if "$@" >"$sandbox/check.out" 2>&1; then
    not_ok "$name"; sed 's/^/      /' "$sandbox/check.out"
  else
    ok "$name"
  fi
}
eq() { [ "$1" = "$2" ] || { printf '      expected: %s\n      got:      %s\n' "$2" "$1"; return 1; }; }
absent() {  # absent NEEDLE FILE...: NEEDLE is in none of the existing FILEs
  local needle="$1" f; shift
  for f in "$@"; do
    if [ -e "$f" ] && grep -rqF -- "$needle" "$f"; then
      printf '      found in %s\n' "$f"; return 1
    fi
  done
}
present() { grep -qF -- "$1" "$2"; }
tmp_empty() { [ -z "$(ls -A "$TMPDIR")" ]; }
home_files() { (cd "$HOME" && find . -type f | LC_ALL=C sort); }
argv_log="$MSW_STUB_STATE/argv.log"
touch "$argv_log"
gh_calls() { if [ -f "$MSW_STUB_STATE/gh-env.log" ]; then wc -l < "$MSW_STUB_STATE/gh-env.log" | tr -d ' '; else echo 0; fi; }
gh_env() { tail -n 1 "$MSW_STUB_STATE/gh-env.log" | jq -r --arg n "$1" '.[$n] // "(unset)"'; }
b64url_decode() {
  local s
  s="$(printf '%s' "$1" | tr '_-' '/+')"
  while [ $((${#s} % 4)) -ne 0 ]; do s="$s="; done
  printf '%s' "$s" | openssl base64 -d -A
}
exits_2() {  # exits_2 COMMAND...: COMMAND fails as bad usage
  local rc=0
  "$@" </dev/null >/dev/null 2>&1 || rc=$?
  eq "$rc" 2
}
fails_saying() {  # fails_saying TEXT COMMAND...: COMMAND fails and its output contains TEXT
  local text="$1"; shift
  if "$@" >"$sandbox/fails.out" 2>&1; then echo "succeeded"; cat "$sandbox/fails.out"; return 1; fi
  cat "$sandbox/fails.out"
  grep -qF -- "$text" "$sandbox/fails.out"
}
curl_calls() { jq -r 'select(.cmd == "curl") | .cmd' "$argv_log" | wc -l | tr -d ' '; }
kc_adds() {  # kc_adds PATTERN: the add-generic-password lines security -i got that match PATTERN
  grep -E -- "^add-generic-password.*$1" "$MSW_STUB_STATE/stdin-security.log" || true
}
fingerprint_of() { openssl pkey -in "$1" -pubout -outform DER | openssl sha256 -binary | openssl base64; }
ident() { git log -1 --format='%an|%ae|%cn|%ce' "$@"; }
S="$here/store-credentials.sh"
M="$here/mint-token.sh"
P="$here/push.sh"
G="$here/bot-gh.sh"
R="$here/request-review.py"

# --- static checks -----------------------------------------------------------

# At severity warning: info and style notes differ between shellcheck versions
# (0.10 on Debian, 0.11 on Homebrew), so they do not fail the check.
check "shellcheck on every script" sh -c 'cd "$1" && shellcheck -S warning -x ./*.sh linux/*.sh' sh "$here"
check "manifest-listener.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$here/manifest-listener.py" "$sandbox/manifest-listener.pyc"
check "request-review.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$R" "$sandbox/request-review.pyc"
check "linux/yq compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$here/linux/yq" "$sandbox/yq.pyc"
no_secret_redirections() {
  local f
  for f in "$here"/*.sh "$here"/linux/*.sh; do
    [ "$f" = "$here/selftest.sh" ] && continue
    if grep -nE '<<|set -x|curl -v' "$f"; then return 1; fi
  done
}
check "no here-string, heredoc or tracing in the tools" no_secret_redirections

# --- per-App table -----------------------------------------------------------

profile_of() {  # profile_of NAME: MSW_PROFILE_* for NAME, |-separated
  (
    unset MSW_STEWARD_CONFIG
    # shellcheck source=common.sh
    . "$here/common.sh"
    msw_app_profile "$1" || exit 1
    printf '%s|%s|%s\n' "$MSW_PROFILE_CONFIG" "$MSW_PROFILE_SERVICE" "$MSW_PROFILE_LABEL"
  )
}
check "the mintswarm-dev profile: dev-app.json, its service and label" \
  eq "$(profile_of mintswarm-dev)" \
     "$MSW_APP_CONFIG|mintswarm-dev-private-key|mintswarm-dev GitHub App private key"
check "the steward profile: steward-app.json, its own service and label" \
  eq "$(profile_of mintswarm-dev-steward)" \
     "$HOME/.config/mintswarm/steward-app.json|mintswarm-dev-steward-private-key|mintswarm-dev-steward GitHub App private key"
steward_config_from_env() {
  (
    MSW_STEWARD_CONFIG="$sandbox/elsewhere.json"
    # shellcheck source=common.sh
    . "$here/common.sh"
    msw_app_profile mintswarm-dev-steward
    printf '%s\n' "$MSW_PROFILE_CONFIG"
  )
}
check "MSW_STEWARD_CONFIG from the environment is taken first" eq "$(steward_config_from_env)" "$sandbox/elsewhere.json"
steward_config_not_derived() {
  (
    unset MSW_STEWARD_CONFIG
    MSW_APP_CONFIG="$sandbox/other/dev-app.json"
    # shellcheck source=common.sh
    . "$here/common.sh"
    msw_app_profile mintswarm-dev-steward
    printf '%s\n' "$MSW_PROFILE_CONFIG"
  )
}
check "the steward's config is never derived from MSW_APP_CONFIG" \
  eq "$(steward_config_not_derived)" "$HOME/.config/mintswarm/steward-app.json"
refuses "an unknown App name has no profile" profile_of mintswarm-other

# --- App creation: create ----------------------------------------------------

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$keys/key1.pem" 2>/dev/null
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$keys/key2.pem" 2>/dev/null
openssl pkey -in "$keys/key1.pem" -pubout -out "$keys/pub1.pem"
openssl pkey -in "$keys/key2.pem" -pubout -out "$keys/pub2.pem"
cp "$keys/key1.pem" "$MSW_STUB_STATE/conv-key.pem"
key1_b64="$(openssl base64 -A -in "$keys/key1.pem")"
key2_b64="$(openssl base64 -A -in "$keys/key2.pem")"
key1_line="$(sed -n 2p "$keys/key1.pem")"
key2_line="$(sed -n 2p "$keys/key2.pem")"

# The manifests in the repository.
check "both manifests are valid JSON" jq -e . "$here/manifests/mintswarm-dev.json" "$here/manifests/mintswarm-dev-steward.json"
dev_description="mintswarm's own control plane while dogfooding: pushes branches and opens PRs, issues and artifacts in mintswarm/mintswarm."
# Name, url, description and events are what App 5874301 was created with; workflows
# was added later, in the App's settings.
dev_manifest_as_before() {
  eq "$(jq -cS '{name, url, description, default_permissions, default_events}' "$here/manifests/mintswarm-dev.json")" \
     "$(jq -cnS --arg d "$dev_description" '{name: "mintswarm-dev", url: "https://github.com/mintswarm/mintswarm", description: $d,
          default_permissions: {contents: "write", pull_requests: "write", issues: "write", workflows: "write",
                                checks: "read", statuses: "read", metadata: "read"},
          default_events: []}')"
}
check "the mintswarm-dev manifest keeps its name, url, description and events, with contents, pull_requests, issues and workflows write and checks, statuses and metadata read" \
  dev_manifest_as_before
check "the steward's permissions are exactly contents and pull_requests write, checks, statuses and metadata read" \
  eq "$(jq -cS .default_permissions "$here/manifests/mintswarm-dev-steward.json")" \
     '{"checks":"read","contents":"write","metadata":"read","pull_requests":"write","statuses":"read"}'
for f in mintswarm-dev mintswarm-dev-steward; do
  check "$f: named after its file, private, no events, no webhook, redirect to 127.0.0.1:8765" \
    jq -e --arg n "$f" '.name == $n and .public == false and .default_events == [] and (has("hook_attributes") | not)
                        and .redirect_url == "http://127.0.0.1:8765/"' "$here/manifests/$f.json"
done

# Helpers for the runs of create.
cat > "$sandbox/setsid.py" <<'EOF'
# setsid.py COMMAND...: run COMMAND as the leader of a new process group, so that
# the whole group, the listener included, can be killed.
import os, sys
os.setsid()
os.execvp(sys.argv[1], sys.argv[1:])
EOF
cat > "$sandbox/probe.py" <<'EOF'
# probe.py page PAGE              the page's form action and manifest, as JSON
# probe.py get PORT TARGET HOST   GET TARGET from 127.0.0.1:PORT with that Host header;
#                                 the status, headers and body as JSON
# probe.py connect HOST PORT      exit 0 if a TCP connection opens within 1 second
# probe.py trickle PORT SECS      connect to 127.0.0.1:PORT, print "connected", then send
#                                 a request one byte every 0.2 seconds for SECS seconds
import html, http.client, json, re, socket, sys, time

cmd = sys.argv[1]
if cmd == "page":
    text = open(sys.argv[2]).read()
    action = html.unescape(re.search(r'<form [^>]*action="([^"]*)"', text).group(1))
    manifest = html.unescape(re.search(r'<input type="hidden" name="manifest" value="([^"]*)"', text).group(1))
    print(json.dumps({"action": action, "manifest": json.loads(manifest),
                      "autosubmit": 'document.getElementById("f").submit()' in text,
                      "button": '<button type="submit">' in text}))
elif cmd == "get":
    conn = http.client.HTTPConnection("127.0.0.1", int(sys.argv[2]), timeout=5)
    conn.putrequest("GET", sys.argv[3], skip_host=True, skip_accept_encoding=True)
    conn.putheader("Host", sys.argv[4])
    conn.endheaders()
    r = conn.getresponse()
    print(json.dumps({"status": r.status, "headers": {k.lower(): v for k, v in r.getheaders()},
                      "body": r.read().decode()}))
elif cmd == "connect":
    host, port = sys.argv[2], int(sys.argv[3])
    try:
        s = socket.socket(socket.AF_INET6 if ":" in host else socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(1)
        s.connect((host, port))
    except OSError:
        sys.exit(1)
    s.close()
elif cmd == "trickle":
    s = socket.create_connection(("127.0.0.1", int(sys.argv[2])), timeout=1)
    print("connected", flush=True)
    end = time.monotonic() + float(sys.argv[3])
    data = b"GET /?code=" + b"a" * 10000
    i = 0
    try:
        while time.monotonic() < end:
            s.sendall(data[i:i + 1])
            i += 1
            time.sleep(0.2)
    except OSError:
        print("dropped", flush=True)
else:
    sys.exit(2)
EOF
probe() { python3 "$sandbox/probe.py" "$@"; }
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'; }
mkdir -p "$sandbox/manifests" "$sandbox/probe"
manifest_copy() {  # manifest_copy NAME [JQ]: manifests/NAME.json redirecting to a free port, then changed by JQ; prints its path
  local port out
  port="$(free_port)"
  out="$sandbox/manifests/$1.$port.json"
  jq --arg r "http://127.0.0.1:$port/" ".redirect_url = \$r | ${2:-.}" "$here/manifests/$1.json" > "$out"
  printf '%s\n' "$out"
}
port_of() { jq -r .redirect_url "$1" | sed 's|^http://127\.0\.0\.1:\([0-9]*\)/$|\1|'; }
conversions() { jq -r 'select(.cmd == "curl") | .argv | select(index("-K")) | "x"' "$argv_log" | wc -l | tr -d ' '; }
create_pid=""
create_out=""
start_create() {  # start_create NAME COMMAND...: COMMAND in the background, in its own process group, with
                  # stdout and stderr to NAME.out and NAME.err; waits at most 10 seconds for the listening line
  local i=0
  create_out="$sandbox/$1"; shift
  python3 "$sandbox/setsid.py" "$@" > "$create_out.out" 2> "$create_out.err" &
  create_pid=$!
  bg_groups="$bg_groups $create_pid"
  while ! grep -q '^manifest-listener.py: listening on ' "$create_out.err" 2>/dev/null; do
    if ! kill -0 "$create_pid" 2>/dev/null; then
      echo "create exited before listening"; cat "$create_out.out" "$create_out.err"; return 1
    fi
    i=$((i + 1))
    if [ "$i" -ge 100 ]; then
      echo "no listening line within 10 seconds"; kill -TERM -- "-$create_pid" 2>/dev/null; return 1
    fi
    sleep 0.1
  done
}
finish_create() {  # finish_create: create's exit status, waiting at most 10 seconds; 124 when it had to be killed
  local i=0 rc=0
  while kill -0 "$create_pid" 2>/dev/null; do
    i=$((i + 1))
    if [ "$i" -ge 100 ]; then
      kill -TERM -- "-$create_pid" 2>/dev/null || true
      wait "$create_pid" 2>/dev/null || true
      return 124
    fi
    sleep 0.1
  done
  wait "$create_pid" || rc=$?
  return "$rc"
}
create_page() { sed -n 's/^manifest-listener.py: listening on .*; open \(.*\) in the browser$/\1/p' "$create_out.err"; }
create_state() { probe page "$(create_page)" | jq -r '.action | sub("^.*[?]state="; "")'; }
accept() {  # accept NAME [HOST]: the matching redirect gets 200 with no-store and no referrer
  probe get "$lport" "/?code=$CODE&state=$state" "${2:-127.0.0.1:$lport}" > "$sandbox/probe/$1.json" || return 1
  cat "$sandbox/probe/$1.json"
  eq "$(jq -c '[.status, .headers["cache-control"], .headers["referrer-policy"], .headers.connection, .body]' "$sandbox/probe/$1.json")" \
     '[200,"no-store","no-referrer","close","received; return to the terminal\n"]'
}
rejected() {  # rejected NAME TARGET [HOST]: 400 with the fixed body, and create keeps running
  probe get "$lport" "$2" "${3:-127.0.0.1:$lport}" > "$sandbox/probe/$1.json" || return 1
  cat "$sandbox/probe/$1.json"
  eq "$(jq -c '[.status, .body]' "$sandbox/probe/$1.json")" '[400,"rejected\n"]' || return 1
  kill -0 "$create_pid"
}
created() {  # created: create exits 0 within 10 seconds
  local rc=0
  finish_create || rc=$?
  cat "$create_out.out" "$create_out.err"
  eq "$rc" 0
}
failed_saying() {  # failed_saying TEXT: create exits non-zero within 10 seconds and its output holds TEXT
  local rc=0
  finish_create || rc=$?
  cat "$create_out.out" "$create_out.err"
  [ "$rc" -ne 0 ] && [ "$rc" -ne 124 ] && grep -qF -- "$1" "$create_out.out" "$create_out.err"
}
refused_early() {  # refused_early TEXT COMMAND...: COMMAND fails saying TEXT before listening, calls no curl, leaves no page
  local text="$1" calls; shift
  calls="$(curl_calls)"
  fails_saying "$text" env MSW_MANIFEST_TIMEOUT=2 "$@" || return 1
  if grep -qF 'listening on' "$sandbox/fails.out"; then echo "it listened"; return 1; fi
  eq "$(curl_calls)" "$calls" || return 1
  eq "$(find "$sandbox" -name 'create-*.html' | wc -l | tr -d ' ')" 0
}
create_argv_start="$(wc -l < "$argv_log" | tr -d ' ')"
create_gh_start="$(gh_calls)"

# Usage and the manifest, the config and the key store, all checked before listening.
check "create without a manifest is bad usage" exits_2 "$S" create
check "create with an unknown option is bad usage" exits_2 "$S" create "$here/manifests/mintswarm-dev.json" --confirm
check "create with --unconfirmed-key is bad usage" \
  exits_2 "$S" create "$here/manifests/mintswarm-dev-steward.json" --unconfirmed-key
check "prepare is gone" exits_2 "$S" prepare
check "convert is gone" exits_2 "$S" convert "$CODE" 0123
check "create refuses a missing manifest" refused_early "no such manifest" "$S" create "$sandbox/manifests/none.json"
printf '[]\n' > "$sandbox/manifests/array.json"
check "create refuses a manifest that is not an object" refused_early "is not a JSON object" "$S" create "$sandbox/manifests/array.json"
check "create refuses an unknown App name" \
  refused_early "no profile for the App 'mintswarm-other'" "$S" create "$(manifest_copy mintswarm-dev '.name = "mintswarm-other"')"
check "create refuses a localhost redirect_url" \
  refused_early "must be http://127.0.0.1:PORT/" "$S" create "$(manifest_copy mintswarm-dev '.redirect_url = "http://localhost:8765/"')"
check "create refuses an https redirect_url" \
  refused_early "must be http://127.0.0.1:PORT/" "$S" create "$(manifest_copy mintswarm-dev '.redirect_url = "https://127.0.0.1:8765/"')"
check "create refuses a redirect_url without a port" \
  refused_early "must be http://127.0.0.1:PORT/" "$S" create "$(manifest_copy mintswarm-dev '.redirect_url = "http://127.0.0.1/"')"
check "create refuses a redirect port below 1024" \
  refused_early "from 1024 to 65535" "$S" create "$(manifest_copy mintswarm-dev '.redirect_url = "http://127.0.0.1:80/"')"
check "create refuses a public App" \
  refused_early '"public": false' "$S" create "$(manifest_copy mintswarm-dev '.public = true')"
check "create refuses an invalid MSW_MANIFEST_TIMEOUT" \
  refused_early "MSW_MANIFEST_TIMEOUT must be a whole number" env MSW_MANIFEST_TIMEOUT=0 MSW_APP_CONFIG="$sandbox/none/dev-app.json" \
  "$S" create "$(manifest_copy mintswarm-dev)"

# mintswarm-dev: the dev-app.json the later checks use.
m_dev="$(manifest_copy mintswarm-dev)"
lport="$(port_of "$m_dev")"
home_files > "$sandbox/home-before"
calls="$(conversions)"
check "create mintswarm-dev: the listener starts" start_create c-dev env MSW_STUB_CONV_MANIFEST="$m_dev" "$S" create "$m_dev"
page="$(create_page)"
check "the page is ~/.config/mintswarm/create-mintswarm-dev.html, mode 0600" \
  eq "$page|$(ls -ld "$page" | cut -c1-10)" "$HOME/.config/mintswarm/create-mintswarm-dev.html|-rw-------"
probe page "$page" > "$sandbox/page-dev.json" || true
state="$(jq -r '.action | sub("^.*[?]state="; "")' "$sandbox/page-dev.json")"
page_action() { jq -r .action "$sandbox/page-dev.json" | grep -qE '^https://github\.com/organizations/mintswarm/settings/apps/new\?state=[0-9a-f]{32}$'; }
check "the page posts to the organisation's new-App form with a random state" page_action
check "the page carries the manifest, submits itself and has a button" \
  eq "$(jq -c --slurpfile m "$m_dev" '[.manifest == $m[0], .autosubmit, .button]' "$sandbox/page-dev.json")" '[true,true,true]'
check "the create output says an owner of the organisation must be signed in" \
  present "signed in as an owner of the mintswarm organisation" "$create_out.out"
check "the state is in no argv" absent "$state" "$argv_log"
check "the matching redirect is accepted" accept dev
check "create mintswarm-dev succeeds" created
check "with one conversion call" eq "$(conversions)" "$((calls + 1))"
home_files > "$sandbox/home-after"
check "HOME gains only dev-app.json, and the page is gone" \
  eq "$(LC_ALL=C comm -13 "$sandbox/home-before" "$sandbox/home-after")" "./.config/mintswarm/dev-app.json"
check "dev-app.json holds the identifiers, the key store and the confirmation kind" \
  eq "$(jq -c 'keys' "$MSW_APP_CONFIG")" '["app_id","client_id","key_confirm","key_fingerprint","key_store","keychain_service","slug"]'
check "dev-app.json: mintswarm-dev's slug and service, the keychain, no confirmation" \
  eq "$(jq -r '[.slug, .keychain_service, .key_store, .key_confirm] | join(" ")' "$MSW_APP_CONFIG")" \
     "mintswarm-dev mintswarm-dev-private-key keychain none"
check "dev-app.json records the key's fingerprint" \
  eq "$(jq -r .key_fingerprint "$MSW_APP_CONFIG")" "SHA256:$(fingerprint_of "$keys/key1.pem")"
check "dev-app.json holds no secret" absent selftest-client-secret-value "$MSW_APP_CONFIG"
check "no secret anywhere under HOME" \
  sh -c '! grep -rqE "selftest-client-secret|selftest-webhook|PRIVATE KEY" "$1"' sh "$HOME"
check "key base64 in no argv" absent "$key1_b64" "$argv_log"
check "key PEM in no argv" absent "$key1_line" "$argv_log"
check "no PRIVATE KEY in any argv" absent "PRIVATE KEY" "$argv_log"
check "key reaches security -i on stdin" present "-w $key1_b64" "$MSW_STUB_STATE/stdin-security.log"
dev_kc_line() {  # mintswarm-dev's item: its label, -U and no -T, as before
  local l
  l="$(kc_adds ' -s mintswarm-dev-private-key ')"
  printf '%s\n' "$l"
  [ "$(printf '%s\n' "$l" | grep -c .)" = 1 ] || return 1
  printf '%s\n' "$l" | grep -qF -- '-U -s mintswarm-dev-private-key ' || return 1
  printf '%s\n' "$l" | grep -qF -- ' -l "mintswarm-dev GitHub App private key" -w ' || return 1
  ! printf '%s\n' "$l" | grep -qF -- ' -T '
}
check "mintswarm-dev's Keychain item: its label quoted, -U and no -T" dev_kc_line
check "TMPDIR is empty after create" tmp_empty

# mintswarm-dev-steward: the listener, and an item and a config of its own.
cp "$MSW_APP_CONFIG" "$sandbox/dev-app.before"
dev_item() { jq -r --arg k "mintswarm-dev-private-key/$CLIENT_ID" '.[$k]' "$MSW_STUB_STATE/keychain.json"; }
dev_item_before="$(dev_item)"
steward_cfg="$HOME/.config/mintswarm/steward-app.json"
m_st="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_st")"
home_files > "$sandbox/home-before"
calls="$(conversions)"
check "create mintswarm-dev-steward: the listener starts" \
  start_create c-st env MSW_STUB_CONV_SLUG=mintswarm-dev-steward MSW_STUB_CONV_MANIFEST="$m_st" "$S" create "$m_st"
state="$(create_state)" || state=""
check "the listening line names 127.0.0.1 and the manifest's port" \
  present "listening on 127.0.0.1:$lport; open $HOME/.config/mintswarm/create-mintswarm-dev-steward.html in the browser" "$create_out.err"
check "the listener answers on 127.0.0.1 (the next two checks are not vacuous)" probe connect 127.0.0.1 "$lport"
refuses "the listener does not answer on [::1]" probe connect ::1 "$lport"
refuses "the listener does not answer on 127.0.0.2 (decisive on Linux)" probe connect 127.0.0.2 "$lport"
check "a wrong state is rejected" rejected wrong-state "/?code=$CODE&state=00000000000000000000000000000000"
check "a missing state is rejected" rejected no-state "/?code=$CODE"
check "an extra parameter is rejected" rejected extra "/?code=$CODE&state=$state&x=1"
check "a repeated parameter is rejected" rejected repeated "/?code=$CODE&code=$CODE&state=$state"
check "another path is rejected" rejected path "/x?code=$CODE&state=$state"
check "a malformed code with the right state is rejected" rejected bad-code "/?code=bad%2Fcode&state=$state"
check "a code with a trailing newline is rejected" rejected nl-code "/?code=$CODE%0A&state=$state"
check "a state with a trailing newline is rejected" rejected nl-state "/?code=$CODE&state=$state%0A"
check "another Host is rejected" rejected host "/?code=$CODE&state=$state" "evil.example:$lport"
reasons_logged() {
  local r
  for r in "wrong state" "unexpected parameters" "wrong path" "malformed code" "unexpected Host"; do
    grep -qF "manifest-listener.py: rejected a request: $r" "$create_out.err" || { echo "no line for: $r"; return 1; }
  done
}
check "each rejection is logged with its reason" reasons_logged
check "no conversion before the matching redirect" eq "$(conversions)" "$calls"
check "the matching redirect is accepted, with localhost as Host" accept steward "localhost:$lport"
check "create mintswarm-dev-steward succeeds" created
check "with exactly one conversion call" eq "$(conversions)" "$((calls + 1))"
refuses "the listener is gone after the conversion" probe connect 127.0.0.1 "$lport"
home_files > "$sandbox/home-after"
check "HOME gains only steward-app.json" \
  eq "$(LC_ALL=C comm -13 "$sandbox/home-before" "$sandbox/home-after")" "./.config/mintswarm/steward-app.json"
check "dev-app.json is byte-identical" cmp "$sandbox/dev-app.before" "$MSW_APP_CONFIG"
check "mintswarm-dev's Keychain item is unchanged" eq "$(dev_item)" "$dev_item_before"
check "steward-app.json holds exactly the identifiers, the key store and the confirmation kind" \
  eq "$(jq -c 'keys' "$steward_cfg")" '["app_id","client_id","key_confirm","key_fingerprint","key_store","keychain_service","slug"]'
check "steward-app.json: the steward's slug and service, the keychain, no confirmation" \
  eq "$(jq -r '[.slug, .keychain_service, .key_store, .key_confirm] | join(" ")' "$steward_cfg")" \
     "mintswarm-dev-steward mintswarm-dev-steward-private-key keychain none"
check "steward-app.json records the key's fingerprint" \
  eq "$(jq -r .key_fingerprint "$steward_cfg")" "SHA256:$(fingerprint_of "$keys/key1.pem")"
check "the stub keychain holds the key under the steward's service" \
  eq "$(jq -r --arg k "mintswarm-dev-steward-private-key/$CLIENT_ID" '.[$k]' "$MSW_STUB_STATE/keychain.json")" "$key1_b64"
steward_kc_line() {  # the steward's item: its label, -U and no -T, as mintswarm-dev's
  local l
  l="$(kc_adds ' -s mintswarm-dev-steward-private-key ')"
  printf '%s\n' "$l"
  [ "$(printf '%s\n' "$l" | grep -c .)" = 1 ] || return 1
  printf '%s\n' "$l" | grep -qF -- '-U -s mintswarm-dev-steward-private-key ' || return 1
  printf '%s\n' "$l" | grep -qF -- ' -l "mintswarm-dev-steward GitHub App private key" -w ' || return 1
  ! printf '%s\n' "$l" | grep -qF -- ' -T '
}
check "the steward's Keychain item: its own label quoted, -U and no -T" steward_kc_line
check "create's output holds no key line" absent "$key1_line" "$sandbox/c-dev.out" "$sandbox/c-dev.err" "$sandbox/c-st.out" "$sandbox/c-st.err"
check "create's output holds no PRIVATE KEY" absent "PRIVATE KEY" "$sandbox/c-dev.out" "$sandbox/c-dev.err" "$sandbox/c-st.out" "$sandbox/c-st.err"
check "create's output holds no client or webhook secret" \
  sh -c '! grep -qE "selftest-client-secret|selftest-webhook" "$@"' sh "$sandbox"/c-dev.* "$sandbox"/c-st.*
check "create prints the slug, id, client id, store, confirmation, fingerprint and config" \
  sh -c 'grep -qF "created: app mintswarm-dev-steward (id 5874301, client $2)" "$1" \
         && grep -qF "key in the keychain key store, confirmation: none" "$1" \
         && grep -qF "key fingerprint SHA256:" "$1" && grep -qF "identifiers: $3" "$1" \
         && grep -qF "README.md, Steward" "$1"' sh "$sandbox/c-st.out" "$CLIENT_ID" "$steward_cfg"

# Refusals once the steward exists.
check "a second steward create is refused because its config exists" \
  refused_early "steward-app.json exists" "$S" create "$(manifest_copy mintswarm-dev-steward)"
check "a second mintswarm-dev create is refused because dev-app.json exists" \
  refused_early "dev-app.json exists" "$S" create "$(manifest_copy mintswarm-dev)"
check "the steward's config set to dev-app.json is refused" \
  refused_early "is mintswarm-dev's config" env MSW_STEWARD_CONFIG="$MSW_APP_CONFIG" "$S" create "$(manifest_copy mintswarm-dev-steward)"
check "the steward's config set to ~/.config/mintswarm/dev-app.json is refused, whatever MSW_APP_CONFIG says" \
  refused_early "is mintswarm-dev's config" env MSW_APP_CONFIG="$sandbox/none/dev-app.json" \
  MSW_STEWARD_CONFIG="$HOME/.config/../.config/mintswarm/dev-app.json" "$S" create "$(manifest_copy mintswarm-dev-steward)"
check "mintswarm-dev's config set to the steward's is refused" \
  refused_early "is the steward's config" env MSW_APP_CONFIG="$steward_cfg" "$S" create "$(manifest_copy mintswarm-dev)"

# A conversion for another App stores nothing.
m_mis="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_mis")"
cp "$MSW_STUB_STATE/keychain.json" "$sandbox/keychain.before"
check "create with a slug mismatch: the listener starts" \
  start_create c-mis env MSW_STEWARD_CONFIG="$sandbox/mis/steward-app.json" MSW_STUB_CONV_SLUG=someone-else \
  MSW_STUB_CONV_MANIFEST="$m_mis" "$S" create "$m_mis"
state="$(create_state)" || state=""
check "its redirect is accepted" accept mis
check "a slug mismatch fails and names the App's id" failed_saying "GitHub created the App someone-else (id 5874301)"
check "and says to delete it" present "delete it (organisation settings, GitHub Apps)" "$create_out.err"
check "and writes no config" test ! -e "$sandbox/mis/steward-app.json"
check "and stores no key" cmp "$sandbox/keychain.before" "$MSW_STUB_STATE/keychain.json"

# Permissions that differ from the manifest only warn: the key can be taken only once.
m_warn="$(manifest_copy mintswarm-dev)"
lport="$(port_of "$m_warn")"
check "create with other permissions in the answer: the listener starts" \
  start_create c-warn env MSW_APP_CONFIG="$sandbox/warn/dev-app.json" MSW_STUB_CONV_MANIFEST="$here/manifests/mintswarm-dev-steward.json" \
  "$S" create "$m_warn"
state="$(create_state)" || state=""
check "its redirect is accepted" accept warn
check "it succeeds" created
check "and warns that the permissions differ" present "warning: mintswarm-dev's permissions differ from the manifest's" "$create_out.err"
check "and writes its config" test -f "$sandbox/warn/dev-app.json"

# Failures after the conversion say what is left behind.
m_put="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_put")"
check "create whose key store refuses the key: the listener starts" \
  start_create c-put env MSW_STUB_SECURITY_WRITE_FAIL=1 MSW_STEWARD_CONFIG="$sandbox/put/steward-app.json" \
  MSW_STUB_CONV_SLUG=mintswarm-dev-steward \
  MSW_STUB_CONV_MANIFEST="$m_put" "$S" create "$m_put"
state="$(create_state)" || state=""
check "its redirect is accepted" accept put
check "it fails, saying the App exists and its key was not stored" \
  failed_saying "the App mintswarm-dev-steward (id 5874301) exists on GitHub, but its key was not stored."
check "and writes no config" test ! -e "$sandbox/put/steward-app.json"
m_rb="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_rb")"
check "create whose read-back fails (as from a locked keychain): the listener starts" \
  start_create c-rb env MSW_STUB_SECURITY_READ_FAIL=1 MSW_STEWARD_CONFIG="$sandbox/rb/steward-app.json" \
  MSW_STUB_CONV_SLUG=mintswarm-dev-steward MSW_STUB_CONV_MANIFEST="$m_rb" "$S" create "$m_rb"
state="$(create_state)" || state=""
check "its redirect is accepted" accept rb
check "it fails, saying the App exists and its key is in the key store" \
  failed_saying "(id 5874301) exists on GitHub and its key is in the key store, but create did not finish."
check "and names the command that deletes the key" \
  present "MSW_KEY_STORE=keychain $here/key-store.sh delete mintswarm-dev-steward-private-key $CLIENT_ID" "$create_out.err"
check "and writes no config" test ! -e "$sandbox/rb/steward-app.json"
check "the listener's line comes before any instruction to open the page" \
  sh -c '! grep -qF "create-mintswarm-dev-steward.html" "$1" && grep -qF "once the listener says it is listening" "$1"' sh "$create_out.out"

# Timeout, and a port held by another socket.
m_to="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_to")"
calls="$(curl_calls)"
check "create with a 2-second timeout: the listener starts" \
  start_create c-to env MSW_MANIFEST_TIMEOUT=2 MSW_STEWARD_CONFIG="$sandbox/to/steward-app.json" "$S" create "$m_to"
page="$(create_page)"
check "with no redirect, create fails within 10 seconds" failed_saying "no matching redirect within 2 seconds"
check "and calls no curl" eq "$(curl_calls)" "$calls"
check "and writes no config" test ! -e "$sandbox/to/steward-app.json"
check "and removes the page" test ! -e "$page"
m_tr="$(manifest_copy mintswarm-dev-steward)"
trickle_bounded() {  # a client sending a byte every 0.2 seconds; create with a 2-second timeout ends within 5
  local start end i=0 tpid rc=0
  lport="$(port_of "$m_tr")"
  start="$(date +%s)"
  start_create c-tr env MSW_MANIFEST_TIMEOUT=2 MSW_STEWARD_CONFIG="$sandbox/tr/steward-app.json" "$S" create "$m_tr" || return 1
  python3 "$sandbox/setsid.py" python3 "$sandbox/probe.py" trickle "$lport" 15 > "$sandbox/trickle.out" 2>&1 &
  tpid=$!
  bg_groups="$bg_groups $tpid"
  until grep -q connected "$sandbox/trickle.out" 2>/dev/null; do
    i=$((i + 1))
    [ "$i" -lt 50 ] || { cat "$sandbox/trickle.out"; return 1; }
    sleep 0.1
  done
  finish_create || rc=$?
  end="$(date +%s)"
  kill -TERM -- "-$tpid" 2>/dev/null || true
  wait "$tpid" 2>/dev/null || true
  cat "$create_out.out" "$create_out.err"
  { [ "$rc" -ne 0 ] && [ "$rc" -ne 124 ]; } || { echo "create's status: $rc"; return 1; }
  grep -qF "no matching redirect within 2 seconds" "$create_out.err" || return 1
  [ $((end - start)) -le 5 ] || { echo "took $((end - start)) seconds"; return 1; }
}
check "a client sending one byte at a time cannot keep the listener past its timeout" trickle_bounded
check "and nothing was converted or written" test ! -e "$sandbox/tr/steward-app.json"
m_held="$(manifest_copy mintswarm-dev-steward)"
held_port="$(port_of "$m_held")"
held_pid=""
hold_port() {  # hold_port PORT: a socket bound to 127.0.0.1:PORT, in the background
  local i=0
  python3 "$sandbox/setsid.py" python3 -c 'import socket, sys, time
s = socket.socket()
s.bind(("127.0.0.1", int(sys.argv[1])))
s.listen(1)
print("held", flush=True)
time.sleep(30)' "$1" > "$sandbox/held.out" 2>&1 &
  held_pid=$!
  bg_groups="$bg_groups $held_pid"
  until grep -q held "$sandbox/held.out" 2>/dev/null; do
    i=$((i + 1))
    [ "$i" -lt 50 ] || { cat "$sandbox/held.out"; return 1; }
    sleep 0.1
  done
}
check "another socket holds a port" hold_port "$held_port"
check "with the port held, create fails before listening" \
  refused_early "cannot listen on 127.0.0.1:$held_port" env MSW_STEWARD_CONFIG="$sandbox/held/steward-app.json" "$S" create "$m_held"
kill -TERM -- "-$held_pid" 2>/dev/null || true
wait "$held_pid" 2>/dev/null || true

# macOS: another key store is taken for the steward as for mintswarm-dev.
mf_env=(HOME="$sandbox/macfile" MSW_KEY_STORE=file)
mkdir -p "$sandbox/macfile"
m_mf="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_mf")"
check "on macOS the steward's create with MSW_KEY_STORE=file: the listener starts" \
  start_create c-mf env "${mf_env[@]}" MSW_STUB_CONV_SLUG=mintswarm-dev-steward MSW_STUB_CONV_MANIFEST="$m_mf" \
  "$S" create "$m_mf"
state="$(create_state)" || state=""
check "its redirect is accepted" accept mf
check "it succeeds" created
check "and records the file store and no confirmation" \
  eq "$(jq -r '[.key_store, .key_confirm] | join(" ")' "$sandbox/macfile/.config/mintswarm/steward-app.json")" "file none"
check "the key is in the file store under the steward's service" \
  cmp "$keys/key1.pem" "$sandbox/macfile/.config/mintswarm/keys/mintswarm-dev-steward-private-key@$CLIENT_ID"

# Linux: the steward is created as on macOS, into the first available store.
lx_env=(HOME="$sandbox/lx" MSW_STUB_UNAME=Linux DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent)
mkdir -p "$sandbox/lx"
m_lx="$(manifest_copy mintswarm-dev-steward)"
lport="$(port_of "$m_lx")"
check "on Linux the steward's create: the listener starts" \
  start_create c-lx env "${lx_env[@]}" MSW_STUB_CONV_SLUG=mintswarm-dev-steward MSW_STUB_CONV_MANIFEST="$m_lx" \
  "$S" create "$m_lx"
state="$(create_state)" || state=""
check "its redirect is accepted" accept lx
check "it succeeds" created
check "and records the secret-service store and no confirmation" \
  eq "$(jq -r '[.key_store, .key_confirm] | join(" ")' "$sandbox/lx/.config/mintswarm/steward-app.json")" "secret-service none"

# Across every run of create.
create_curls() { tail -n "+$((create_argv_start + 1))" "$argv_log" | jq -c 'select(.cmd == "curl") | .argv'; }
check "every curl call of create is a conversion POST that reads its URL from stdin" \
  eq "$(create_curls | sort -u)" '["-q","-fsS","-X","POST","-H","Accept: application/vnd.github+json","-K","-"]'
check "and the only URL curl got is the conversion's" \
  sh -c '! grep -v -e "^url = \"https://api.github.com/app-manifests/$2/conversions\"\$" -e "^--\$" -e "^\$" "$1"' \
  sh "$MSW_STUB_STATE/stdin-curl.log" "$CODE"
check "the code reached curl on stdin" present "app-manifests/$CODE/conversions" "$MSW_STUB_STATE/stdin-curl.log"
check "create made no gh call" eq "$(gh_calls)" "$create_gh_start"
check "the code is in no output of create, no argv, no response and nothing under HOME or TMPDIR" \
  absent "$CODE" "$sandbox"/c-*.out "$sandbox"/c-*.err "$argv_log" "$sandbox/probe" "$HOME" "$sandbox/lx" "$sandbox/macfile" "$TMPDIR"
check "TMPDIR is empty after the create runs" tmp_empty

# --- JWT and installation token ----------------------------------------------

verify_jwt() {  # verify_jwt PUBKEY
  local jwt h p s
  jwt="$("$M" jwt)" || return 1
  h="${jwt%%.*}"; s="${jwt##*.}"; p="${jwt#*.}"; p="${p%.*}"
  printf '%s.%s' "$h" "$p" > "$sandbox/jwt.input"
  b64url_decode "$s" > "$sandbox/jwt.sig"
  openssl dgst -sha256 -verify "$1" -signature "$sandbox/jwt.sig" "$sandbox/jwt.input" >/dev/null || return 1
  b64url_decode "$p" > "$sandbox/jwt.payload"
  eq "$(jq -r .iss "$sandbox/jwt.payload")" "$CLIENT_ID" || return 1
  eq "$(jq -r '.exp - .iat' "$sandbox/jwt.payload")" 600
}
check "JWT verifies with the App key and iss is the client id" verify_jwt "$keys/pub1.pem"

token_narrowed() {
  local tok body
  tok="$("$M" token '{"contents":"read"}')" || return 1
  eq "$tok" "$MINTED" || return 1
  body="$(jq -r 'select(.cmd == "curl") | .argv | index("-d") as $i | if $i then .[$i + 1] else empty end' "$argv_log" | tail -n 1)"
  eq "$(printf '%s' "$body" | jq -c .)" '{"repositories":["mintswarm"],"permissions":{"contents":"read"}}'
}
check "token returns the minted token with narrowed permissions" token_narrowed
check "the JWT is in no argv" absent "Bearer" "$argv_log"
check "the JWT reaches curl on stdin" present "Authorization: Bearer " "$MSW_STUB_STATE/stdin-curl.log"
check "TMPDIR is empty after minting" tmp_empty
refuses "token fails when the API fails" env MSW_STUB_CURL_FAIL=1 "$M" token
refuses "token fails without dev-app.json" env MSW_APP_CONFIG="$sandbox/missing.json" "$M" token
refuses "token refuses non-object permissions" "$M" token '"contents"'
calls="$(curl_calls)"
refuses "a bare mint-token.sh prints usage, not a token" "$M"
check "a bare mint-token.sh calls nothing" eq "$(curl_calls)" "$calls"
check "a bare mint-token.sh prints no token" absent "$MINTED" "$sandbox/check.out"

# --- key rotation ------------------------------------------------------------

mkdir -p "$HOME/Downloads"
download="$HOME/Downloads/mintswarm-dev.2026-11-22.private-key.pem"
printf 'not a key\n' > "$download"
refuses "rotate refuses a file that is not a private key" "$S" rotate "$download"
check "rotate keeps a refused file" test -f "$download"
cp "$keys/key2.pem" "$download"
home_files > "$sandbox/home-before"
check "rotate --keep succeeds and keeps the file" sh -c '"$1" rotate "$2" --keep >/dev/null && test -f "$2"' sh "$S" "$download"
check "rotate succeeds" sh -c '"$1" rotate "$2" > "$3"' sh "$S" "$download" "$sandbox/rotate.out"
check "rotate deletes the downloaded PEM" test ! -e "$download"
home_files > "$sandbox/home-after"
check "rotate leaves HOME with no new file" \
  eq "$(LC_ALL=C comm -13 "$sandbox/home-before" "$sandbox/home-after")" ""
check "rotate updates the fingerprint" \
  eq "$(jq -r .key_fingerprint "$MSW_APP_CONFIG")" "SHA256:$(fingerprint_of "$keys/key2.pem")"
check "rotate keeps the client id" eq "$(jq -r .client_id "$MSW_APP_CONFIG")" "$CLIENT_ID"
check "the rotated key signs the JWT" verify_jwt "$keys/pub2.pem"
check "rotated key base64 in no argv" absent "$key2_b64" "$argv_log"
check "rotated key PEM in no argv" absent "$key2_line" "$argv_log"
check "TMPDIR is empty after rotate" tmp_empty
check "rotate records mintswarm-dev's confirmation kind" eq "$(jq -r .key_confirm "$MSW_APP_CONFIG")" none
dev_rotated_line() {
  local l
  l="$(kc_adds ' -s mintswarm-dev-private-key ' | tail -n 1)"
  printf '%s\n' "$l"
  printf '%s\n' "$l" | grep -qF -- '-U -s mintswarm-dev-private-key ' || return 1
  printf '%s\n' "$l" | grep -qF -- ' -l "mintswarm-dev GitHub App private key" -w '"$key2_b64" || return 1
  ! printf '%s\n' "$l" | grep -qF -- ' -T '
}
check "rotate keeps mintswarm-dev's label, -U and no -T" dev_rotated_line

# The steward's key: no confirmation, its own item and config, rotated while the item exists.
cp "$MSW_APP_CONFIG" "$sandbox/dev-app.before"
dev_item_before="$(dev_item)"
st_rotate() { env MSW_APP_CONFIG="$steward_cfg" "$S" rotate "$@"; }
cp "$keys/key2.pem" "$sandbox/steward-new.pem"
check "rotating the steward's key succeeds while its item exists" st_rotate "$sandbox/steward-new.pem"
steward_rotated_line() {
  local l
  l="$(kc_adds ' -s mintswarm-dev-steward-private-key ' | tail -n 1)"
  printf '%s\n' "$l"
  printf '%s\n' "$l" | grep -qF -- '-U -s mintswarm-dev-steward-private-key ' || return 1
  printf '%s\n' "$l" | grep -qF -- ' -l "mintswarm-dev-steward GitHub App private key" -w '"$key2_b64" || return 1
  ! printf '%s\n' "$l" | grep -qF -- ' -T '
}
check "it sends -U with the steward's label and no -T" steward_rotated_line
check "it records none and the new fingerprint" \
  eq "$(jq -r '[.key_confirm, .key_store, .key_fingerprint] | join(" ")' "$steward_cfg")" \
     "none keychain SHA256:$(fingerprint_of "$keys/key2.pem")"
steward_jwt() { ( export MSW_APP_CONFIG="$steward_cfg"; verify_jwt "$keys/pub2.pem" ); }
check "the steward's rotated key signs its JWT" steward_jwt
jq 'del(.key_confirm)' "$steward_cfg" > "$sandbox/steward-no-confirm.json"
cp "$keys/key2.pem" "$sandbox/steward-no-confirm.pem"
check "a steward config without key_confirm rotates" \
  env MSW_APP_CONFIG="$sandbox/steward-no-confirm.json" "$S" rotate "$sandbox/steward-no-confirm.pem"
check "as none" eq "$(jq -r .key_confirm "$sandbox/steward-no-confirm.json")" none
jq '.key_confirm = "keychain-acl"' "$steward_cfg" > "$sandbox/steward-acl.json"
cp "$sandbox/steward-acl.json" "$sandbox/steward-acl.before"
cp "$MSW_STUB_STATE/keychain.json" "$sandbox/keychain.before"
cp "$keys/key1.pem" "$sandbox/steward-acl.pem"
check "a config recording keychain-acl is refused, saying to delete the item and set none" \
  fails_saying "delete the key-store item (key-store.sh delete mintswarm-dev-steward-private-key $CLIENT_ID) and set key_confirm to none" \
  env MSW_APP_CONFIG="$sandbox/steward-acl.json" "$S" rotate "$sandbox/steward-acl.pem"
check "and stores nothing, leaving the item, the config and the file" \
  sh -c 'cmp "$1" "$2" && cmp "$3" "$4" && test -f "$5"' sh "$sandbox/keychain.before" "$MSW_STUB_STATE/keychain.json" \
  "$sandbox/steward-acl.before" "$sandbox/steward-acl.json" "$sandbox/steward-acl.pem"
check "after the steward's create and rotate, dev-app.json is unchanged" cmp "$sandbox/dev-app.before" "$MSW_APP_CONFIG"
check "and so is mintswarm-dev's Keychain item" eq "$(dev_item)" "$dev_item_before"
lx_cfg="$sandbox/lx/.config/mintswarm/steward-app.json"
cp "$keys/key2.pem" "$sandbox/lx-new.pem"
check "on Linux the steward's key rotates as none" \
  env "${lx_env[@]}" MSW_APP_CONFIG="$lx_cfg" "$S" rotate "$sandbox/lx-new.pem"
check "and stays in secret-service with no confirmation" \
  eq "$(jq -r '[.key_store, .key_confirm] | join(" ")' "$lx_cfg")" "secret-service none"
cp "$keys/key1.pem" "$sandbox/any.pem"
jq '.slug = "mintswarm-other"' "$steward_cfg" > "$sandbox/other-slug.json"
check "rotate refuses a config naming an unknown App" \
  fails_saying "names no known App (slug 'mintswarm-other'" env MSW_APP_CONFIG="$sandbox/other-slug.json" "$S" rotate "$sandbox/any.pem"
jq 'del(.slug)' "$MSW_APP_CONFIG" > "$sandbox/no-slug.json"
check "rotate refuses a config without a slug" \
  fails_saying "names no known App (slug ''" env MSW_APP_CONFIG="$sandbox/no-slug.json" "$S" rotate "$sandbox/any.pem"
jq '.keychain_service = "mintswarm-dev-private-key"' "$steward_cfg" > "$sandbox/wrong-service.json"
check "rotate refuses a keychain_service that is not the App's" \
  fails_saying "but mintswarm-dev-steward's is mintswarm-dev-steward-private-key" \
  env MSW_APP_CONFIG="$sandbox/wrong-service.json" "$S" rotate "$sandbox/any.pem"
jq 'del(.key_confirm)' "$MSW_APP_CONFIG" > "$sandbox/dev-no-confirm.json"
cp "$keys/key2.pem" "$sandbox/same.pem"   # the key mintswarm-dev already has, so the later checks still sign with key2
dev_no_confirm() {
  env MSW_APP_CONFIG="$sandbox/dev-no-confirm.json" "$S" rotate "$sandbox/same.pem" || return 1
  eq "$(jq -r .key_confirm "$sandbox/dev-no-confirm.json")" none
}
check "a mintswarm-dev config without key_confirm rotates as before, as none" dev_no_confirm
check "the refused rotations kept the file" test -f "$sandbox/any.pem"
check "TMPDIR is empty after the steward's rotation" tmp_empty

# --- key store ---------------------------------------------------------------

K="$here/key-store.sh"
ks_root="$sandbox/ks"   # HOME and stores for these checks, outside the main HOME
mkdir -p "$ks_root/sel" "$ks_root/home" "$ks_root/pass" "$ks_root/xdg" "$ks_root/creds"
printf 'selftest-gpg-id\n' > "$ks_root/pass/.gpg-id"

with_value() { printf 'abc\n' | "$@"; }   # with_value COMMAND...: COMMAND with a dummy value on stdin
linux_sel() { env HOME="$ks_root/sel" MSW_STUB_UNAME=Linux "$@"; }
no_key_dir() { test ! -e "$ks_root/sel/.config/mintswarm/keys"; }

refused_calls() { jq -r 'select(.cmd == "security") | .argv[0]' "$argv_log" | grep -c -e delete -e svc || true; }
selection_calls() { jq -r 'select(.cmd == "secret-tool" or .cmd == "pass") | .cmd' "$argv_log" | wc -l | tr -d ' '; }
refused_before="$(refused_calls)"
selection_before="$(selection_calls)"
for n in '' .. .x -x a/b 'a b' 'a$b'; do
  check "key-store.sh refuses the service name '$n' as bad usage" exits_2 env MSW_KEY_STORE=keychain "$K" get "$n" acct
  check "key-store.sh refuses the account name '$n' as bad usage" exits_2 env MSW_KEY_STORE=keychain "$K" get svc "$n"
done
check "key-store.sh refuses a label with a quote" exits_2 env MSW_KEY_STORE=keychain "$K" put svc acct 'a"b'
check "key-store.sh without arguments is bad usage" exits_2 "$K"
check "key-store.sh refuses an extra argument" exits_2 "$K" get svc acct extra
check "refused names reached no key store" eq "$(refused_calls)" "$refused_before"

check "Darwin selects keychain" eq "$(env HOME="$ks_root/sel" "$K" backend 2>&1)" keychain
check "Linux with a session bus address selects secret-service" \
  eq "$(linux_sel DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent "$K" backend 2>&1)" secret-service
python3 -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$ks_root/xdg/bus"
check "Linux with a socket at XDG_RUNTIME_DIR/bus selects secret-service" \
  eq "$(linux_sel XDG_RUNTIME_DIR="$ks_root/xdg" "$K" backend 2>&1)" secret-service
check "secret-service comes before pass" \
  eq "$(linux_sel DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent PASSWORD_STORE_DIR="$ks_root/pass" "$K" backend 2>&1)" secret-service
check "Linux with an initialised pass store selects pass" \
  eq "$(linux_sel PASSWORD_STORE_DIR="$ks_root/pass" "$K" backend 2>&1)" pass
printf 'x\n' > "$ks_root/creds/msw-selftest@sel"
chmod 400 "$ks_root/creds/msw-selftest@sel"
check "Linux with the credential in CREDENTIALS_DIRECTORY selects systemd-creds" \
  eq "$(linux_sel CREDENTIALS_DIRECTORY="$ks_root/creds" "$K" backend msw-selftest sel 2>&1)" systemd-creds
refuses "CREDENTIALS_DIRECTORY selects systemd-creds only for the credential it holds" \
  linux_sel CREDENTIALS_DIRECTORY="$ks_root/creds" "$K" backend msw-selftest other
check "Linux with no key store fails and names every MSW_KEY_STORE value" \
  fails_saying "set MSW_KEY_STORE to one of keychain, secret-service, pass, systemd-creds, file" linux_sel "$K" backend
check "put with no key store fails" fails_saying "MSW_KEY_STORE" with_value linux_sel "$K" put msw-selftest sel
check "an unknown MSW_KEY_STORE fails" fails_saying "unknown MSW_KEY_STORE 'vault'" linux_sel MSW_KEY_STORE=vault "$K" backend
check "an explicit secret-service without a bus fails" \
  fails_saying "MSW_KEY_STORE=secret-service is not available" with_value linux_sel MSW_KEY_STORE=secret-service "$K" put msw-selftest sel
check "an explicit keychain fails on Linux" fails_saying "MSW_KEY_STORE=keychain is not available" linux_sel MSW_KEY_STORE=keychain "$K" backend
check "an explicit pass without an initialised store fails" \
  fails_saying "MSW_KEY_STORE=pass is not available" linux_sel MSW_KEY_STORE=pass "$K" backend
check "create with no key store fails before listening and calls no GitHub endpoint" \
  refused_early "set MSW_KEY_STORE to one of" env HOME="$ks_root/sel" MSW_STUB_UNAME=Linux \
  MSW_APP_CONFIG="$ks_root/sel/.config/mintswarm/dev-app.json" "$S" create "$(manifest_copy mintswarm-dev)"
check "the file backend was never used without MSW_KEY_STORE=file" no_key_dir
check "the failed selections ran no secret-tool or pass" eq "$(selection_calls)" "$selection_before"

ks_vars() {  # ks_vars BACKEND: export the environment that makes BACKEND available
  export MSW_KEY_STORE="$1" HOME="$ks_root/home"
  if [ "$1" != keychain ]; then export MSW_STUB_UNAME=Linux; fi
  case "$1" in
    secret-service) export DBUS_SESSION_BUS_ADDRESS="unix:path=$sandbox/no-such-bus" ;;
    pass) export PASSWORD_STORE_DIR="$ks_root/pass" ;;
  esac
}
ks() {  # ks BACKEND COMMAND...: COMMAND in BACKEND's environment
  local b="$1"; shift
  ( ks_vars "$b"; "$@" )
}
ks_roundtrip() {  # ks_roundtrip BACKEND ACCOUNT: put, get byte for byte, delete, get fails
  ks "$1" "$K" put msw-selftest "$2" < "$keys/key1.pem" || return 1
  ks "$1" "$K" get msw-selftest "$2" > "$ks_root/get.out" || return 1
  cmp "$keys/key1.pem" "$ks_root/get.out" || return 1
  ks "$1" "$K" delete msw-selftest "$2" || return 1
  if ks "$1" "$K" get msw-selftest "$2" > /dev/null; then echo "get after delete succeeded"; return 1; fi
}
ks_e2e() {  # ks_e2e BACKEND: rotate into a copy of dev-app.json through BACKEND, then sign a JWT
  local cfg="$ks_root/dev-app.$1.json"
  cp "$MSW_APP_CONFIG" "$cfg"
  cp "$keys/key1.pem" "$ks_root/rotate.pem"
  ks "$1" env MSW_APP_CONFIG="$cfg" "$S" rotate "$ks_root/rotate.pem" > "$ks_root/rotate.$1.out" || return 1
  cat "$ks_root/rotate.$1.out"
  eq "$(jq -r .key_store "$cfg")" "$1" || return 1
  eq "$(jq -r .key_fingerprint "$cfg")" "SHA256:$(fingerprint_of "$keys/key1.pem")" || return 1
  grep -qF "MSW_KEY_STORE=keychain $K delete mintswarm-dev-private-key $CLIENT_ID" "$ks_root/rotate.$1.out" || return 1
  ( ks_vars "$1"; unset MSW_KEY_STORE; export MSW_APP_CONFIG="$cfg"; verify_jwt "$keys/pub1.pem" )
}
argv_has() { jq -c 'select(.cmd == "'"$1"'") | .argv' "$argv_log" | grep -qF -- "$2"; }   # argv_has CMD TEXT

# keychain
check "keychain: round trip, delete, get after delete fails" ks_roundtrip keychain kc
check "keychain: the default label is used" present '-l "mintswarm msw-selftest kc"' "$MSW_STUB_STATE/stdin-security.log"
check "keychain: delete goes through delete-generic-password" argv_has security '["delete-generic-password","-s","msw-selftest","-a","kc"]'
kc_no_trust_flags() {  # the plain put's line has neither -T nor a missing -U
  local l
  l="$(kc_adds ' -a kc ')"
  [ -n "$l" ] || { echo "no add for kc"; return 1; }
  printf '%s\n' "$l"
  ! printf '%s\n' "$l" | grep -qF -- ' -T ' && printf '%s\n' "$l" | grep -qF -- ' -U '
}
check "keychain: put without --confirm sends -U and no -T" kc_no_trust_flags
check "keychain: put --confirm succeeds" ks keychain "$K" put --confirm msw-selftest cf < "$keys/key1.pem"
kc_confirm_line() {
  local l
  l="$(kc_adds ' -a cf ')"
  printf '%s\n' "$l"
  [ "$(printf '%s\n' "$l" | grep -c .)" = 1 ] || return 1
  printf '%s\n' "$l" | grep -qF -- ' -l "mintswarm msw-selftest cf" -T "" -w ' || return 1
  ! printf '%s\n' "$l" | grep -qF -- ' -U '
}
check "keychain: put --confirm sends -T \"\" and no -U" kc_confirm_line
confirm_get() { ks keychain "$K" get msw-selftest cf > "$ks_root/get.out" && cmp "$keys/key1.pem" "$ks_root/get.out"; }
check "keychain: the confirmed item reads back byte for byte" confirm_get
check "keychain: put --confirm refuses an existing item and says to delete it first" \
  fails_saying "already exists in the keychain store: delete it first" ks keychain "$K" put --confirm msw-selftest cf < "$keys/key2.pem"
check "keychain: the refused put reached no add" eq "$(kc_adds ' -a cf ' | grep -c .)" 1
check "keychain: and left the item alone" confirm_get
check "keychain: the existence check reads no value" \
  argv_has security '["find-generic-password","-s","msw-selftest","-a","cf"]'
ks keychain "$K" delete msw-selftest cf
check "secret-service: put --confirm is refused" \
  fails_saying "per-use confirmation is available only in the keychain store" \
  ks secret-service "$K" put --confirm msw-selftest cf < "$keys/key1.pem"
check "file: put --confirm is refused" \
  fails_saying "per-use confirmation is available only in the keychain store" \
  ks file "$K" put --confirm msw-selftest cf < "$keys/key1.pem"
check "the refused --confirm puts stored nothing" \
  sh -c '! grep -qF "service/msw-selftest/account/cf" "$1"/secret-service.json 2>/dev/null && test ! -e "$2"' sh \
  "$MSW_STUB_STATE" "$ks_root/home/.config/mintswarm/keys/msw-selftest@cf"
check "put --confirm without an account is bad usage" exits_2 env MSW_KEY_STORE=keychain "$K" put --confirm msw-selftest
check "put --confirm twice is bad usage" exits_2 env MSW_KEY_STORE=keychain "$K" put --confirm --confirm msw-selftest cf
jq 'del(.key_store)' "$MSW_APP_CONFIG" > "$ks_root/legacy.json"
legacy_jwt() { ( export MSW_APP_CONFIG="$ks_root/legacy.json"; verify_jwt "$keys/pub2.pem" ); }
check "a dev-app.json without key_store signs with the Keychain on macOS" legacy_jwt
check "a dev-app.json without key_store fails on Linux and says how to choose" \
  fails_saying "key_store missing from $ks_root/legacy.json: store the key here with MSW_KEY_STORE=" \
  env MSW_STUB_UNAME=Linux MSW_APP_CONFIG="$ks_root/legacy.json" "$M" jwt
cp "$keys/key1.pem" "$ks_root/legacy.pem"
check "rotate with such a file fails on Linux without MSW_KEY_STORE" \
  fails_saying "key_store missing" env MSW_STUB_UNAME=Linux MSW_APP_CONFIG="$ks_root/legacy.json" "$S" rotate "$ks_root/legacy.pem"
check "the refused rotate kept the file and left dev-app.json alone" \
  sh -c 'test -f "$1" && ! jq -e .key_store "$2" >/dev/null' sh "$ks_root/legacy.pem" "$ks_root/legacy.json"
mint_ignores_env() {
  ( export MSW_KEY_STORE=file; verify_jwt "$keys/pub2.pem" ) 2> "$ks_root/mint.err" || return 1
  if grep -q unencrypted "$ks_root/mint.err"; then cat "$ks_root/mint.err"; return 1; fi
}
check "mint-token.sh ignores MSW_KEY_STORE and reads the recorded key store" mint_ignores_env

# secret-service
check "secret-service: round trip, delete, get after delete fails" ks_roundtrip secret-service ss
check "secret-service: the key reaches secret-tool store on stdin" present "$key1_line" "$MSW_STUB_STATE/stdin-secret-tool.log"
check "secret-service: the default label is used" argv_has secret-tool '"--label=mintswarm msw-selftest ss"'
check "secret-service: rotate, then mint-token.sh jwt" ks_e2e secret-service
check "secret-service: rotate passes the key label" argv_has secret-tool '"--label=mintswarm-dev GitHub App private key"'
hangs_briefly() {  # hangs_briefly OP: with secret-tool hanging, OP fails within 5 seconds and the stub is gone
  local start end pid
  rm -f "$MSW_STUB_STATE/secret-tool.pid"
  start="$(date +%s)"
  if ks secret-service env MSW_STUB_SECRET_TOOL_HANG=1 MSW_KEY_STORE_TIMEOUT=1 \
       sh -c 'printf "abc\n" | "$@"' sh "$K" "$1" msw-selftest hang > "$ks_root/hang.out" 2>&1; then
    echo "$1 succeeded"; return 1
  fi
  end="$(date +%s)"
  cat "$ks_root/hang.out"
  [ $((end - start)) -lt 5 ] || { echo "took $((end - start)) seconds"; return 1; }
  grep -qF "secret-tool did not answer within 1 seconds: the keyring is probably locked; unlock it first" "$ks_root/hang.out" \
    || return 1
  pid="$(cat "$MSW_STUB_STATE/secret-tool.pid")" || return 1
  if kill -0 "$pid" 2>/dev/null; then echo "secret-tool ($pid) is still running"; return 1; fi
}
check "secret-service: with the keyring locked, get fails promptly and says to unlock it" hangs_briefly get
check "secret-service: with the keyring locked, put fails promptly and says to unlock it" hangs_briefly put
check "secret-service: a locked keyring falls back to nothing" test ! -e "$ks_root/home/.config/mintswarm/keys"
check "an invalid MSW_KEY_STORE_TIMEOUT is bad usage" exits_2 ks secret-service env MSW_KEY_STORE_TIMEOUT=soon "$K" backend
ks secret-service "$K" put msw-selftest tty < "$keys/key1.pem"
tty_refused() {
  if ks secret-service python3 "$sandbox/on-tty.py" "$K" get msw-selftest tty > "$ks_root/tty.out" 2> "$ks_root/tty.err"; then
    echo "get to a terminal succeeded"; return 1
  fi
  cat "$ks_root/tty.err"
  grep -qF "refusing to print msw-selftest/tty to a terminal" "$ks_root/tty.err" || return 1
  if grep -qF -e "$key1_line" -e "PRIVATE KEY" "$ks_root/tty.out"; then echo "the key reached the terminal"; return 1; fi
}
check "get refuses to print the value to a terminal" tty_refused
tty_on_pipe() { ks secret-service python3 "$sandbox/on-tty.py" sh -c '"$1" get msw-selftest tty | wc -c' sh "$K"; }
check "get to a pipe works when the caller's stdout is a terminal (the test is not vacuous)" tty_on_pipe
ks secret-service "$K" delete msw-selftest tty
check "TMPDIR is empty after the secret-service checks" tmp_empty

# pass
check "pass: round trip, delete, get after delete fails" ks_roundtrip pass pa
check "pass: the key reaches pass insert on stdin" present "$key1_line" "$MSW_STUB_STATE/stdin-pass.log"
check "pass: the entry is mintswarm/SERVICE/ACCOUNT" argv_has pass '["insert","-m","-f","mintswarm/msw-selftest/pa"]'
check "pass: rotate, then mint-token.sh jwt" ks_e2e pass
check "TMPDIR is empty after the pass checks" tmp_empty

# systemd-creds
cred="$ks_root/home/.config/mintswarm/credstore/msw-selftest@sd.cred"
check "systemd-creds: put" ks systemd-creds "$K" put msw-selftest sd < "$keys/key1.pem"
check "systemd-creds: the key reaches encrypt on stdin" present "$key1_line" "$MSW_STUB_STATE/stdin-systemd-creds.log"
check "systemd-creds: the .cred file exists" test -f "$cred"
check "systemd-creds: the .cred file holds no key line" absent "$key1_line" "$cred"
check "systemd-creds: the .cred file holds no PRIVATE KEY" absent "PRIVATE KEY" "$cred"
if [ "$(id -u)" != 0 ]; then
  check "systemd-creds: --user is passed when not root" argv_has systemd-creds '["--user","encrypt","--name=msw-selftest@sd","-",'
  refuses "systemd-creds: unavailable for a non-root user below systemd 256" \
    ks systemd-creds env MSW_STUB_SYSTEMD_VERSION=255 "$K" backend
else
  check "systemd-creds: --user is not passed as root" argv_has systemd-creds '["encrypt","--name=msw-selftest@sd","-",'
fi
sd_get() { ks systemd-creds "$K" get msw-selftest sd > "$ks_root/get.out" && cmp "$keys/key1.pem" "$ks_root/get.out"; }
check "systemd-creds: get decrypts it byte for byte" sd_get
check "systemd-creds: delete removes the .cred file" sh -c '"$@" && test ! -e "$0"' "$cred" \
  env HOME="$ks_root/home" MSW_STUB_UNAME=Linux MSW_KEY_STORE=systemd-creds "$K" delete msw-selftest sd
refuses "systemd-creds: get after delete fails" ks systemd-creds "$K" get msw-selftest sd
cp "$keys/key1.pem" "$ks_root/creds/msw-selftest@cd"
chmod 400 "$ks_root/creds/msw-selftest@cd"
decrypts() { jq -r 'select(.cmd == "systemd-creds") | .argv | map(select(. == "decrypt")) | .[]' "$argv_log" | wc -l | tr -d ' '; }
calls="$(decrypts)"
sd_passed() {
  ks systemd-creds env CREDENTIALS_DIRECTORY="$ks_root/creds" MSW_STUB_SYSTEMD_VERSION=255 \
    "$K" get msw-selftest cd > "$ks_root/get.out" && cmp "$keys/key1.pem" "$ks_root/get.out"
}
check "systemd-creds: get reads \$CREDENTIALS_DIRECTORY first" sd_passed
check "systemd-creds: and does not call decrypt for it" eq "$(decrypts)" "$calls"
check "systemd-creds: rotate, then mint-token.sh jwt" ks_e2e systemd-creds
check "TMPDIR is empty after the systemd-creds checks" tmp_empty

# file
fdir="$ks_root/home/.config/mintswarm/keys"
file_put_warns() {
  ks file "$K" put msw-selftest fl < "$keys/key1.pem" 2> "$ks_root/file.err" || { cat "$ks_root/file.err"; return 1; }
  grep -qF "warning: the file key store keeps the key unencrypted in $fdir/msw-selftest@fl" "$ks_root/file.err"
}
file_get_warns() {
  ks file "$K" get msw-selftest fl > "$ks_root/get.out" 2> "$ks_root/file.err" || { cat "$ks_root/file.err"; return 1; }
  cmp "$keys/key1.pem" "$ks_root/get.out" || return 1
  grep -qF "warning: the file key store keeps the key unencrypted" "$ks_root/file.err"
}
check "file: put stores the key and warns that it is unencrypted" file_put_warns
check "file: the key file is 0600" eq "$(ls -ld "$fdir/msw-selftest@fl" | cut -c1-10)" "-rw-------"
check "file: its directory is 0700" eq "$(ls -ld "$fdir" | cut -c1-10)" "drwx------"
check "file: get returns it byte for byte and warns" file_get_warns
chmod 640 "$fdir/msw-selftest@fl"
check "file: get refuses a key file at 0640" fails_saying "group or other permissions" ks file "$K" get msw-selftest fl
refuses "file: put refuses a key file at 0640" ks file "$K" put msw-selftest fl < "$keys/key2.pem"
check "file: the refused put left the file alone" cmp "$keys/key1.pem" "$fdir/msw-selftest@fl"
chmod 600 "$fdir/msw-selftest@fl"
chmod 750 "$fdir"
check "file: get refuses a key directory at 0750" fails_saying "group or other permissions" ks file "$K" get msw-selftest fl
chmod 700 "$fdir"
cp "$keys/key2.pem" "$ks_root/elsewhere.pem"
chmod 600 "$ks_root/elsewhere.pem"
ln -s "$ks_root/elsewhere.pem" "$fdir/msw-selftest@ln"
check "file: get refuses a symlinked key file" fails_saying "symlink" ks file "$K" get msw-selftest ln
refuses "file: put refuses a symlinked key file" ks file "$K" put msw-selftest ln < "$keys/key1.pem"
check "file: nothing was written through the symlink" cmp "$keys/key2.pem" "$ks_root/elsewhere.pem"
mkdir -p "$ks_root/home-ln/.config/mintswarm" "$ks_root/real-keys"
chmod 700 "$ks_root/real-keys"
ln -s "$ks_root/real-keys" "$ks_root/home-ln/.config/mintswarm/keys"
check "file: put refuses a symlinked key directory" \
  fails_saying "symlink" ks file env HOME="$ks_root/home-ln" "$K" put msw-selftest fl < "$keys/key1.pem"
check "file: nothing was written into the symlinked directory" eq "$(ls -A "$ks_root/real-keys")" ""
check "file: delete removes the key file" \
  sh -c '"$@" 2>/dev/null && test ! -e "$0"' "$fdir/msw-selftest@fl" \
  env HOME="$ks_root/home" MSW_STUB_UNAME=Linux MSW_KEY_STORE=file "$K" delete msw-selftest fl
refuses "file: get after delete fails" ks file "$K" get msw-selftest fl
mkdir -p "$ks_root/home-cfg/.config" "$ks_root/dotfiles/mintswarm"
ln -s "$ks_root/dotfiles/mintswarm" "$ks_root/home-cfg/.config/mintswarm"
check "file: put refuses a symlinked ~/.config/mintswarm" \
  fails_saying "mintswarm: it is a symlink (a dotfiles link?)" \
  ks file env HOME="$ks_root/home-cfg" "$K" put msw-selftest fl < "$keys/key1.pem"
mkdir -p "$ks_root/home-cfg2" "$ks_root/dotfiles/config"
ln -s "$ks_root/dotfiles/config" "$ks_root/home-cfg2/.config"
check "file: put refuses a symlinked ~/.config" \
  fails_saying ".config: it is a symlink (a dotfiles link?)" \
  ks file env HOME="$ks_root/home-cfg2" "$K" put msw-selftest fl < "$keys/key1.pem"
check "file: nothing was written into the dotfiles" eq "$(find "$ks_root/dotfiles" -type f | wc -l | tr -d ' ')" 0
mkdir -p "$ks_root/home-git"
git init -q "$ks_root/home-git"
check "file: put refuses a key directory inside a git work tree" \
  fails_saying "is inside a git work tree" \
  ks file env HOME="$ks_root/home-git" "$K" put msw-selftest fl < "$keys/key1.pem"
check "file: nothing was written into the work tree" test ! -e "$ks_root/home-git/.config/mintswarm/keys"
mkdir -p "$ks_root/home-git/.config/mintswarm/keys"
chmod 700 "$ks_root/home-git/.config/mintswarm/keys"
cp "$keys/key1.pem" "$ks_root/home-git/.config/mintswarm/keys/msw-selftest@fl"
chmod 600 "$ks_root/home-git/.config/mintswarm/keys/msw-selftest@fl"
check "file: get refuses a key file inside a git work tree" \
  fails_saying "is inside a git work tree" ks file env HOME="$ks_root/home-git" "$K" get msw-selftest fl
check "file: rotate, then mint-token.sh jwt" ks_e2e file
check "file: rotate left only the key file in the key directory" \
  eq "$(ls -A "$fdir" | LC_ALL=C sort | tr '\n' ' ')" "mintswarm-dev-private-key@$CLIENT_ID msw-selftest@ln "

check "key lines in no argv after the key store checks" absent "$key1_line" "$argv_log"
check "key base64 in no argv after the key store checks" absent "$key1_b64" "$argv_log"
check "no PRIVATE KEY in any argv after the key store checks" absent "PRIVATE KEY" "$argv_log"
check "TMPDIR is empty after the key store checks" tmp_empty

# --- bot identity ------------------------------------------------------------

remote="$sandbox/remote.git"
repo="$sandbox/work"
git init -q --bare "$remote"
git init -q -b master "$repo"
cd "$repo"
git config user.name Maintainer
git config user.email maintainer@example.com
git commit -q --allow-empty -m base
git push -q "$remote" master
git remote add origin "$remote"
git fetch -q origin

"$here/bot-identity.sh" install > /dev/null
"$here/bot-identity.sh" install > /dev/null
check "install is idempotent" \
  eq "$(git config --local --get-all 'includeIf.onbranch:mintswarm/.path')" "$HOME/.config/mintswarm/bot-identity.gitconfig"
check "status reports the include" sh -c '"$1" status | grep -q "^include:   /"' sh "$here/bot-identity.sh"

git checkout -q -b mintswarm/1-x
printf 'a\n' > a.txt
git add a.txt
git commit -q -m "bot commit"
check "a commit on mintswarm/1-x is the bot's" eq "$(ident)" "$BOT_IDENT"
git worktree add -q -b mintswarm/2-y "$sandbox/wt" master
(cd "$sandbox/wt" && printf 'b\n' > b.txt && git add b.txt && git commit -q -m "worktree commit")
check "a commit in a linked worktree on mintswarm/2-y is the bot's" eq "$(ident mintswarm/2-y)" "$BOT_IDENT"
git checkout -q master
git commit -q --allow-empty -m "master moves"
check "a commit on master is not the bot's" eq "$(ident master)" "$MAINT|$MAINT"
git push -q origin master
git fetch -q origin
git checkout -q mintswarm/1-x
git rebase -q master
check "a commit rewritten by a rebase is the bot's" eq "$(ident)" "$BOT_IDENT"
git checkout -q -b mintswarm/7-exec
git rebase -q --exec 'git commit -q --allow-empty -m during-rebase' master >/dev/null 2>&1
check "a commit from a separate git process during a rebase is not the bot's (documented limit)" \
  eq "$(ident)" "$MAINT|$MAINT"
check "push.sh refuses that commit" \
  sh -c '! MSW_TOKEN="$1" MSW_PUSH_URL="file://$2" "$3" mintswarm/7-exec >/dev/null 2>&1' sh "$DUMMY" "$remote" "$here/push.sh"
git checkout -q --detach
git commit -q --allow-empty -m detached
check "a commit on a detached HEAD is not the bot's (documented limit)" eq "$(ident)" "$MAINT|$MAINT"
git checkout -q mintswarm/1-x

# --- push.sh -----------------------------------------------------------------

url="file://$remote"
cp .git/config "$sandbox/config-before"
remote_sha() { git --git-dir="$remote" rev-parse --verify --quiet "refs/heads/$1" || true; }

check "push.sh pushes bot commits" env MSW_TOKEN="$DUMMY" MSW_PUSH_URL="$url" "$P" mintswarm/1-x
check "the pushed branch is at the local commit" eq "$(remote_sha mintswarm/1-x)" "$(git rev-parse mintswarm/1-x)"
check "push.sh mints its own token when MSW_TOKEN is unset" env MSW_PUSH_URL="$url" "$P" mintswarm/2-y
push_perms() {
  jq -r 'select(.cmd == "curl") | .argv | index("-d") as $i | if $i then .[$i + 1] else empty end' "$argv_log" \
    | tail -n 1 | jq -c .permissions
}
check "push.sh narrows its token to contents:write" eq "$(push_perms)" '{"contents":"write"}'

git branch feat/1-x mintswarm/1-x
git branch --no-track mintswarm/5-empty refs/remotes/origin/master
git checkout -q --no-track -b mintswarm/3-author refs/remotes/origin/master
GIT_AUTHOR_NAME=Maintainer GIT_AUTHOR_EMAIL=maintainer@example.com git commit -q --allow-empty -m "maintainer author"
git checkout -q --no-track -b mintswarm/4-committer refs/remotes/origin/master
GIT_COMMITTER_NAME=Maintainer GIT_COMMITTER_EMAIL=maintainer@example.com git commit -q --allow-empty -m "maintainer committer"
git checkout -q mintswarm/1-x
cp .git/config "$sandbox/config-before"

push_as_bot() { env MSW_TOKEN="$DUMMY" MSW_PUSH_URL="$url" "$P" "$@"; }
refuses "push.sh refuses master" push_as_bot master
refuses "push.sh refuses a branch outside mintswarm/" push_as_bot feat/1-x
refuses "push.sh refuses a missing branch" push_as_bot mintswarm/9-missing
refuses "push.sh refuses a maintainer-authored commit" push_as_bot mintswarm/3-author
refuses "push.sh refuses a maintainer-committed commit" push_as_bot mintswarm/4-committer
refuses "push.sh refuses an empty range" push_as_bot mintswarm/5-empty
refuses "push.sh refuses a bare --force-with-lease" push_as_bot mintswarm/1-x --force-with-lease
refuses "push.sh refuses --force" push_as_bot mintswarm/1-x --force
refuses "push.sh refuses a short lease" push_as_bot mintswarm/1-x --force-with-lease=abc123
refuses "push.sh refuses a non-file MSW_PUSH_URL" \
  env MSW_TOKEN="$DUMMY" MSW_PUSH_URL=https://example.com/x.git "$P" mintswarm/1-x
refuses "push.sh refuses a token that is not ghs_" \
  env MSW_TOKEN=ghp_notaninstallationtoken MSW_PUSH_URL="$url" "$P" mintswarm/1-x
origin_master="$(git rev-parse refs/remotes/origin/master)"
git update-ref -d refs/remotes/origin/master
refuses "push.sh refuses without origin/master" push_as_bot mintswarm/1-x
git update-ref refs/remotes/origin/master "$origin_master"

old="$(git rev-parse mintswarm/1-x)"
git checkout -q --no-track -B mintswarm/1-x refs/remotes/origin/master
git commit -q --allow-empty -m "rewritten"
refuses "a non-fast-forward push is rejected without a lease" push_as_bot mintswarm/1-x
refuses "a push with a stale lease is rejected" push_as_bot mintswarm/1-x "--force-with-lease=$origin_master"
check "a push with the right lease succeeds" push_as_bot mintswarm/1-x "--force-with-lease=$old"
check "the leased push moved the branch" eq "$(remote_sha mintswarm/1-x)" "$(git rev-parse mintswarm/1-x)"
check "refused branches never reached the remote" \
  eq "$(git --git-dir="$remote" for-each-ref --format='%(refname)' | tr '\n' ' ')" \
     "refs/heads/master refs/heads/mintswarm/1-x refs/heads/mintswarm/2-y "
check ".git/config is byte-identical after pushing" cmp -s .git/config "$sandbox/config-before"
check "the token is nowhere under .git, the remote or HOME" absent "$DUMMY" "$repo/.git" "$remote" "$HOME"
check "the minted token is nowhere under .git, the remote or HOME" absent "$MINTED" "$repo/.git" "$remote" "$HOME"
check "TMPDIR is empty after pushing" tmp_empty

# The token gets workflows:write only when the range from the merge base touches .github/workflows/.
git checkout -q --no-track -b mintswarm/6-workflow refs/remotes/origin/master
mkdir -p .github/workflows
printf 'name: ci\n' > .github/workflows/ci.yml
git add .github/workflows/ci.yml
git commit -q -m "add a workflow"
check "push.sh pushes a branch that adds a workflow" env MSW_PUSH_URL="$url" "$P" mintswarm/6-workflow
check "push.sh asks for workflows:write when the branch touches .github/workflows/" \
  eq "$(push_perms)" '{"contents":"write","workflows":"write"}'
git checkout -q --no-track -b mintswarm/9-rebase refs/remotes/origin/master
printf 'c\n' > c.txt
git add c.txt
git commit -q -m "no workflow"
check "push.sh pushes mintswarm/9-rebase" env MSW_PUSH_URL="$url" "$P" mintswarm/9-rebase
rebase_old="$(git rev-parse mintswarm/9-rebase)"
git checkout -q --detach refs/remotes/origin/master
mkdir -p .github/workflows
printf 'name: other\n' > .github/workflows/other.yml
git add .github/workflows/other.yml
git commit -q -m "maintainer adds a workflow on master"
git update-ref refs/remotes/origin/master HEAD
check "push.sh pushes mintswarm/2-y with a workflow commit on origin/master only" \
  env MSW_PUSH_URL="$url" "$P" mintswarm/2-y
check "workflow commits on origin/master alone do not widen the token" eq "$(push_perms)" '{"contents":"write"}'
git checkout -q mintswarm/9-rebase
git rebase -q refs/remotes/origin/master
check "push.sh refuses a lease commit that is not here" \
  fails_saying "fetch the branch's old tip first" \
  push_as_bot mintswarm/9-rebase --force-with-lease=0123456789abcdef0123456789abcdef01234567
check "push.sh pushes a branch rebased onto a workflow change, with the old tip as lease" \
  env MSW_PUSH_URL="$url" "$P" mintswarm/9-rebase "--force-with-lease=$rebase_old"
check "a rebase onto a workflow change on origin/master asks for workflows:write" \
  eq "$(push_perms)" '{"contents":"write","workflows":"write"}'
git checkout -q --no-track -b mintswarm/10-delete refs/remotes/origin/master
git rm -q .github/workflows/other.yml
git commit -q -m "delete a workflow"
check "push.sh pushes a branch that deletes a workflow" env MSW_PUSH_URL="$url" "$P" mintswarm/10-delete
check "deleting a workflow asks for workflows:write" eq "$(push_perms)" '{"contents":"write","workflows":"write"}'
git update-ref refs/remotes/origin/master "$origin_master"
git checkout -q mintswarm/1-x

git config url.git@github.com:.insteadOf https://github.com/
refuses "push.sh refuses a url.*.insteadOf in the repository config" push_as_bot mintswarm/2-y
git config --unset url.git@github.com:.insteadOf
git config credential.helper store
refuses "push.sh refuses a credential helper in the repository config" push_as_bot mintswarm/2-y
git config --unset credential.helper
git config http.https://github.com/.extraheader "AUTHORIZATION: basic eA=="
refuses "push.sh refuses an extra header in the repository config" push_as_bot mintswarm/2-y
git config --unset http.https://github.com/.extraheader
git config http.proxy http://127.0.0.1:9
refuses "push.sh refuses http.proxy in the repository config" push_as_bot mintswarm/2-y
git config --unset http.proxy
git config http.sslVerify false
refuses "push.sh refuses http.sslVerify in the repository config" push_as_bot mintswarm/2-y
git config --unset http.sslVerify
git config http.https://github.com/.sslCAInfo /tmp/ca.pem
refuses "push.sh refuses http.<url>.sslCAInfo in the repository config" push_as_bot mintswarm/2-y
git config --unset http.https://github.com/.sslCAInfo

# A remote section named after the URL given to git push applies to that push.
other="$sandbox/other.git"
git init -q --bare "$other"
git config "remote.$url.pushurl" "file://$other"
refuses "push.sh refuses remote.<push URL>.pushurl" push_as_bot mintswarm/2-y
git config --unset "remote.$url.pushurl"
git config "remote.$url.url" "file://$other"
refuses "push.sh refuses remote.<push URL>.url" push_as_bot mintswarm/2-y
git config --unset "remote.$url.url"
git config remote.https://github.com/mintswarm/mintswarm.git.pushurl git@github.com:mintswarm/mintswarm.git
refuses "push.sh refuses an ssh pushurl for the GitHub URL" push_as_bot mintswarm/2-y
git config --unset remote.https://github.com/mintswarm/mintswarm.git.pushurl
# The legacy GIT_CONFIG changes what git config reads, not what git push reads.
git config "remote.$url.pushurl" "file://$other"
: > "$sandbox/empty.gitconfig"
check "GIT_CONFIG hides a planted pushurl from a plain git config (the test is not vacuous)" \
  sh -c '! GIT_CONFIG="$1" git config --get-regexp "^remote\\." >/dev/null' sh "$sandbox/empty.gitconfig"
refuses "push.sh refuses remote.<push URL>.pushurl with GIT_CONFIG set" \
  env GIT_CONFIG="$sandbox/empty.gitconfig" MSW_TOKEN="$DUMMY" MSW_PUSH_URL="$url" "$P" mintswarm/2-y
git config --unset "remote.$url.pushurl"
check "nothing reached the redirect target" eq "$(git --git-dir="$other" for-each-ref | wc -l | tr -d ' ')" "0"
git_env_sees() {  # git_env_sees: the environment msw_git_env gives a git alias
  (
    # shellcheck source=common.sh
    . "$here/common.sh"
    GIT_CONFIG="$sandbox/empty.gitconfig" GIT_SSL_NO_VERIFY=1 GIT_SSL_CAINFO=/tmp/ca.pem GIT_SSL_CAPATH=/tmp \
      msw_git_env -c 'alias.e=!env' e
  )
}
git_env_clean() {
  git_env_sees > "$sandbox/git-env.out" || return 1
  grep -q '^GIT_CONFIG_NOSYSTEM=1$' "$sandbox/git-env.out" || { echo "the alias did not run"; return 1; }
  ! grep -E '^(GIT_CONFIG|GIT_SSL_NO_VERIFY|GIT_SSL_CAINFO|GIT_SSL_CAPATH)=' "$sandbox/git-env.out"
}
check "msw_git_env unsets GIT_CONFIG and GIT_SSL_*" git_env_clean

# Behind the refusal, msw_git_net allows only https (and file when the script says so).
printf '#!/bin/sh\necho "ssh $*" >> "$MSW_STUB_STATE/ssh.log"\nexit 1\n' > "$stubs/ssh"
chmod 755 "$stubs/ssh"
net_push() {  # net_push ALLOW_FILE URL REFSPEC: msw_git_net push without push.sh's checks
  (
    # shellcheck source=common.sh
    . "$here/common.sh"
    MSW_GIT_ALLOW_FILE="$1"
    MSW_TOKEN="$DUMMY"; export MSW_TOKEN
    msw_git_net push --no-verify "$2" "$3"
  )
}
redirect_blocked() {  # redirect_blocked PUSHURL: a planted redirect to PUSHURL fails before ssh runs
  local rc=0
  git config "remote.$url.pushurl" "$1"
  GIT_SSH_COMMAND="$stubs/ssh" GIT_ALLOW_PROTOCOL=ssh:file:https net_push 1 "$url" \
    refs/heads/mintswarm/2-y:refs/heads/redirected > "$sandbox/net.out" 2>&1 || rc=$?
  git config --unset "remote.$url.pushurl"
  cat "$sandbox/net.out"
  [ "$rc" -ne 0 ] && grep -q "transport 'ssh' not allowed" "$sandbox/net.out" && [ ! -e "$MSW_STUB_STATE/ssh.log" ]
}
check "a planted ssh:// redirect fails without running ssh" redirect_blocked ssh://git@ssh.invalid/x.git
check "a planted scp-style redirect fails without running ssh" redirect_blocked git@ssh.invalid:x.git
check "the redirects reached neither remote" \
  eq "$(git --git-dir="$remote" for-each-ref refs/heads/redirected; git --git-dir="$other" for-each-ref)" ""
file_blocked() {  # MSW_GIT_ALLOW_FILE=1 in the environment is reset when common.sh is sourced
  if (
    export MSW_GIT_ALLOW_FILE=1
    # shellcheck source=common.sh
    . "$here/common.sh"
    MSW_TOKEN="$DUMMY"; export MSW_TOKEN
    msw_git_net push --no-verify "$url" refs/heads/mintswarm/2-y:refs/heads/by-file
  ) > "$sandbox/net.out" 2>&1; then
    return 1
  fi
  cat "$sandbox/net.out"
  grep -q "transport 'file' not allowed" "$sandbox/net.out"
}
check "file is not allowed unless the script allows it, whatever the environment says" file_blocked

# Hooks never run, so none sees MSW_TOKEN.
for d in .git/hooks "$sandbox/hooks"; do
  mkdir -p "$d"
  printf '#!/bin/sh\necho "ran ${MSW_TOKEN:-no-token}" >> "%s/hook.log"\n' "$sandbox" > "$d/pre-push"
  chmod 755 "$d/pre-push"
done
git push -q "$remote" mintswarm/2-y:refs/heads/hook-check
check "a planted pre-push hook runs for a plain push (the test is not vacuous)" grep -q "ran no-token" "$sandbox/hook.log"
rm -f "$sandbox/hook.log"
git --git-dir="$remote" update-ref -d refs/heads/hook-check
git checkout -q --no-track -b mintswarm/8-hooks mintswarm/1-x
git commit -q --allow-empty -m "hook check 1"
check "push.sh pushes with a pre-push hook in .git/hooks" push_as_bot mintswarm/8-hooks
git config core.hooksPath "$sandbox/hooks"
git commit -q --allow-empty -m "hook check 2"
check "push.sh pushes with core.hooksPath set" push_as_bot mintswarm/8-hooks
git config --unset core.hooksPath
check "no hook ran during push.sh" test ! -e "$sandbox/hook.log"
rm -f .git/hooks/pre-push
git checkout -q mintswarm/1-x

# --- credential helper -------------------------------------------------------

git init -q "$sandbox/cred"
cd "$sandbox/cred"
git config credential.https://github.com.helper '!f() { echo username=evil-url; echo password=evil-url; }; f'
git config credential.helper '!f() { echo username=evil-local; echo password=evil-local; }; f'
printf '[credential]\n\thelper = "!f() { echo username=evil-global; echo password=evil-global; }; f"\n' > "$HOME/.gitconfig"
planted="$(printf 'protocol=https\nhost=github.com\n\n' | git credential fill 2>/dev/null || true)"
check "the planted helpers answer a plain git (the test is not vacuous)" present evil <(printf '%s\n' "$planted")
filled="$(printf 'protocol=https\nhost=github.com\n\n' | (
  # shellcheck source=common.sh
  . "$here/common.sh"
  MSW_TOKEN="$DUMMY"; export MSW_TOKEN
  msw_git_net credential fill
))"
check "credential fill gives x-access-token" eq "$(printf '%s\n' "$filled" | grep '^username=')" "username=x-access-token"
check "credential fill gives the token" eq "$(printf '%s\n' "$filled" | grep '^password=')" "password=$DUMMY"
check "no planted helper answered" absent evil <(printf '%s\n' "$filled")
net_fill() {  # net_fill PROTOCOL HOST: what msw_git_net's helper answers
  printf 'protocol=%s\nhost=%s\n\n' "$1" "$2" | (
    # shellcheck source=common.sh
    . "$here/common.sh"
    MSW_TOKEN="$DUMMY"; export MSW_TOKEN
    msw_git_net credential fill 2>/dev/null
  ) || true
}
check "credential fill gives no token for another https host" absent "$DUMMY" <(net_fill https example.com)
check "credential fill gives no token for http://github.com" absent "$DUMMY" <(net_fill http github.com)
check "credential fill gives no token for a host that only starts with github.com" \
  absent "$DUMMY" <(net_fill https github.com.example.com)
rm -f "$HOME/.gitconfig"
cd "$repo"

# --- bot-gh.sh ---------------------------------------------------------------

calls="$(gh_calls)"
check "bot-gh.sh runs gh" \
  env GITHUB_TOKEN=ghp_maintainer GH_ENTERPRISE_TOKEN=x GITHUB_ENTERPRISE_TOKEN=y MSW_TOKEN="$DUMMY" \
  "$G" api 'repos/{owner}/{repo}' --jq .full_name
check "gh sees GH_TOKEN" eq "$(gh_env GH_TOKEN)" "$DUMMY"
check "gh sees no GITHUB_TOKEN" eq "$(gh_env GITHUB_TOKEN)" "(unset)"
check "gh sees no enterprise tokens" eq "$(gh_env GH_ENTERPRISE_TOKEN)$(gh_env GITHUB_ENTERPRISE_TOKEN)" "(unset)(unset)"
check "gh sees GH_REPO=mintswarm/mintswarm" eq "$(gh_env GH_REPO)" "mintswarm/mintswarm"
check "gh sees GH_HOST=github.com" eq "$(gh_env GH_HOST)" "github.com"
check "gh sees prompts disabled" eq "$(gh_env GH_PROMPT_DISABLED)" "1"
check "the token is in no argv" absent "$DUMMY" "$argv_log"
check "bot-gh.sh mints its own token" "$G" api 'repos/{owner}/{repo}'
check "gh sees the minted token" eq "$(gh_env GH_TOKEN)" "$MINTED"
check "the minted token is in no argv" absent "$MINTED" "$argv_log"
calls="$(gh_calls)"
refuses "bot-gh.sh fails when minting fails" env MSW_STUB_CURL_FAIL=1 "$G" api 'repos/{owner}/{repo}'
refuses "bot-gh.sh fails without dev-app.json" env MSW_APP_CONFIG="$sandbox/missing.json" "$G" api x
refuses "bot-gh.sh refuses a token that is not ghs_" env MSW_TOKEN=ghp_maintainer "$G" api x
check "gh is not run when there is no installation token" eq "$(gh_calls)" "$calls"

# --- request-review.py -------------------------------------------------------

for f in crates/core/x.rs Cargo.toml .mintswarm/stages/plan.md; do
  check "reviewers for $f from the real config" eq "$("$R" 1 --dry-run --files "$f")" "zmij"
done
cat > "$sandbox/review.yaml" <<'EOF'
components:
  core: {paths: ["crates/core/**"], reviewers: [alice, "@bob"]}
  docs: {paths: ["docs/**", "README.md"]}
  deep: {paths: ["a/**/b.md", "*.toml", "x?.txt"], reviewers: [carol]}
pipeline:
  review: {reviewers: [zmij]}
EOF
rr() { "$R" 1 --dry-run --config "$sandbox/review.yaml" "$@" | tr '\n' ' '; }
check "component reviewers, @ stripped" eq "$(rr --files crates/core/x.rs)" "alice bob "
check "union with the default for a component without reviewers" eq "$(rr --files crates/core/x.rs docs/a.md)" "alice bob zmij "
check "**/ matches zero directories" eq "$(rr --files a/b.md)" "carol "
check "**/ matches several directories" eq "$(rr --files a/x/y/b.md)" "carol "
check "* stays within a segment" eq "$(rr --files x/Cargo.toml)" "zmij "
check "* matches at the top level" eq "$(rr --files Cargo.toml)" "carol "
check "? matches one character" eq "$(rr --files x1.txt x12.txt)" "carol zmij "
check "a prefix is not a match" eq "$(rr --files crates/corex/y.rs)" "zmij "
printf 'components:\n  core: {paths: ["crates/core/**"], reviewers: ["al ice"]}\n' > "$sandbox/bad.yaml"
bad_login() { "$R" 1 --dry-run --config "$sandbox/bad.yaml" --files crates/core/x.rs >/dev/null 2>&1; [ $? -eq 2 ]; }
check "an invalid login exits 2" bad_login
printf 'components:\n  core: {paths: ["crates/core/**"]}\n' > "$sandbox/none.yaml"
check "no reviewers resolved is a no-op" eq "$("$R" 1 --config "$sandbox/none.yaml" --files docs/a.md 2>/dev/null)" ""

git checkout -q --no-track -b mintswarm/6-review refs/remotes/origin/master
mkdir -p crates/core docs
printf 'x\n' > crates/core/a.rs
printf 'x\n' > docs/b.md
git add crates/core/a.rs docs/b.md
git commit -q -m "review files"
check "changed files come from git diff BASE...BRANCH" \
  eq "$(rr --branch mintswarm/6-review --base refs/remotes/origin/master)" "alice bob zmij "

requested="$(env MSW_TOKEN="$DUMMY" "$R" 7 --files crates/core/x.rs)" || requested="(failed)"
check "request-review.py prints the requested reviewers" eq "$requested" "zmij"
last_gh_argv() { jq -c 'select(.cmd == "gh") | .argv' "$argv_log" | tail -n 1; }
check "request-review.py posts to requested_reviewers through bot-gh.sh" \
  eq "$(last_gh_argv)" '["api","--method","POST","repos/{owner}/{repo}/pulls/7/requested_reviewers","--input","-"]'
check "request-review.py sends the reviewers on stdin" present '{"reviewers": ["zmij"]}' "$MSW_STUB_STATE/stdin-gh.log"
printf '{"7": [{"id": 1, "user": {"login": "zmij", "type": "User"}, "state": "CHANGES_REQUESTED", "submitted_at": "2026-11-23T10:00:00Z"}]}\n' \
  > "$MSW_STUB_STATE/reviews.json"
rereview_calls_before="$(gh_calls)"
rerequested="$(env MSW_TOKEN="$DUMMY" "$R" 7 --files crates/core/x.rs)" || rerequested="(failed)"
rerequests_once() {  # exactly one new gh call, the POST to requested_reviewers, naming zmij on stdin
  eq "$(( $(gh_calls) - rereview_calls_before ))" 1 &&
    eq "$(last_gh_argv)" '["api","--method","POST","repos/{owner}/{repo}/pulls/7/requested_reviewers","--input","-"]' &&
    eq "$(tail -n 2 "$MSW_STUB_STATE/stdin-gh.log" | head -n 1)" '{"reviewers": ["zmij"]}' &&
    eq "$rerequested" "zmij"
}
check "re-requests a reviewer who has already reviewed" rerequests_once

# --- assign-approvers.py -----------------------------------------------------

A="$here/assign-approvers.py"
check "assign-approvers.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$A" "$sandbox/assign-approvers.pyc"
check "assign-approvers.py names no approver" absent zmij "$A"
aa() { python3 -I "$A" "$@"; }
assignees() { jq -r --arg n "$1" '.[$n].assignees | join(" ")' "$MSW_STUB_STATE/issues.json"; }
seed() {  # seed N JSON: issue N's state in the gh stub
  local f="$MSW_STUB_STATE/issues.json"
  [ -f "$f" ] || printf '{}\n' > "$f"
  jq --arg n "$1" --argjson v "$2" '.[$n] = $v' "$f" > "$f.new" && mv "$f.new" "$f"
}
gh_argv_seen() { jq -c 'select(.cmd == "gh") | .argv' "$argv_log" | grep -qxF -- "$1"; }
deletes() { jq -c 'select(.cmd == "gh") | .argv | select(index("DELETE"))' "$argv_log" | wc -l | tr -d ' '; }
assign_drops() {  # assign on issue 13, where bob cannot be assigned: exit 1, naming bob
  local rc=0
  aa assign 13 plan --base HEAD >"$sandbox/aa.out" 2>&1 || rc=$?
  cat "$sandbox/aa.out"
  eq "$rc" 1 && grep -qF "did not assign bob" "$sandbox/aa.out"
}
commit_config() {  # commit_config YAML: commit YAML as .mintswarm/config.yaml and print the commit
  printf '%s\n' "$1" > .mintswarm/config.yaml
  git add .mintswarm/config.yaml
  git commit -q -m "config"
  git rev-parse HEAD
}
posts() { jq -c 'select(.cmd == "gh") | .argv | select(index("POST"))' "$argv_log" | wc -l | tr -d ' '; }
last_stdin() { tail -n 2 "$MSW_STUB_STATE/stdin-gh.log" | head -n 1; }

git checkout -q --no-track -b mintswarm/7-assign refs/remotes/origin/master
mkdir -p .mintswarm
cp "$here/../../config.yaml" .mintswarm/config.yaml
git add .mintswarm/config.yaml
git commit -q -m "the real config"
real_base="$(git rev-parse HEAD)"
bots_base="$(commit_config 'pipeline: {review: {reviewers: ["mintswarm-dev[bot]", "other[bot]"]}}')"
list_base="$(commit_config 'pipeline: [review]')"
newline_base="$(commit_config 'pipeline: {review: {reviewers: ["alice\n"]}}')"
commit_config 'pipeline: {review: {reviewers: [alice, "@bob", "mintswarm-dev[bot]", "other[bot]", alice, Alice]}}' >/dev/null
check "approvers from the real config" eq "$(aa assign 1 plan --dry-run --base "$real_base")" "zmij"
check "assign with no human approver exits 2" exits_2 aa assign 1 plan --base "$bots_base"
check "a pipeline that is not a mapping exits 2" exits_2 aa assign 1 plan --base "$list_base"
check "a login with a trailing newline exits 2" exits_2 aa assign 1 plan --base "$newline_base"
check "approvers from the base config: @ stripped, bots skipped, deduplicated without regard to case" \
  eq "$(aa assign 1 plan --dry-run --base HEAD 2>/dev/null | tr '\n' ' ')" "alice bob "
check "a skipped bot is noted" present "skipping other[bot]" <(aa assign 1 plan --dry-run --base HEAD 2>&1)
printf 'pipeline:\n  review: {reviewers: [mallory]}\n' > .mintswarm/config.yaml
check "an uncommitted edit of the config changes nothing" \
  eq "$(aa assign 1 plan --dry-run --base HEAD 2>/dev/null | tr '\n' ' ')" "alice bob "
git checkout -q -- .mintswarm/config.yaml
check "an unknown reason exits 2" exits_2 aa assign 1 later --base HEAD
check "number 0 exits 2" exits_2 aa assign 0 plan --base HEAD
check "a missing action exits 2" exits_2 aa 1 plan

out="$(aa assign 11 plan --base HEAD 2>/dev/null)" || out="(failed)"
check "assign prints the assigned logins and the reason" eq "$out" "assigned alice bob to #11 (plan)"
check "assign posts to the issue's assignees through bot-gh.sh" \
  eq "$(last_gh_argv)" '["api","--method","POST","repos/{owner}/{repo}/issues/11/assignees","--input","-"]'
check "gh sees the minted token" eq "$(gh_env GH_TOKEN)" "$MINTED"
check "assign reads the issue first" gh_argv_seen '["api","repos/{owner}/{repo}/issues/11"]'
check "assign sends the approvers on stdin" eq "$(last_stdin)" '{"assignees": ["alice", "bob"]}'
check "no bot login is in any request body" absent "[bot]" "$MSW_STUB_STATE/stdin-gh.log"
check "issue 11 is assigned to alice and bob" eq "$(assignees 11)" "alice bob"
seed 13 '{"assignees": [], "events": [], "unassignable": ["bob"]}'
check "a login GitHub drops fails assign with 1 and is named" assign_drops

out="$(aa release 11 pr 2>/dev/null)" || out="(failed)"
check "release prints the released logins and the reason" eq "$out" "released alice bob from #11 (pr)"
check "release reads every page of the events" \
  gh_argv_seen '["api","--paginate","--slurp","repos/{owner}/{repo}/issues/11/events"]'
check "release sends DELETE to the issue's assignees" \
  eq "$(last_gh_argv)" '["api","--method","DELETE","repos/{owner}/{repo}/issues/11/assignees","--input","-"]'
check "release sends the bot's assignees on stdin" \
  eq "$(last_stdin)" '{"assignees": ["alice", "bob"]}'
check "issue 11 has no assignees left" eq "$(assignees 11)" ""

seed 12 '{"assignees": ["carol"], "events": [{"event": "assigned", "actor": {"login": "carol"}, "assignee": {"login": "carol"}}]}'
aa assign 12 plan --base HEAD >/dev/null 2>&1 || true
check "issue 12 is assigned to carol, alice and bob" eq "$(assignees 12)" "carol alice bob"
check "release --dry-run prints what it would remove" eq "$(aa release 12 plan --dry-run 2>/dev/null | tr '\n' ' ')" "alice bob "
out="$(aa release 12 plan 2>/dev/null)" || out="(failed)"
check "release removes only the bot's assignments" eq "$out" "released alice bob from #12 (plan)"
check "carol, assigned by a person, stays" eq "$(assignees 12)" "carol"
seed 14 '{"assignees": ["alice"], "events": [{"event": "assigned", "actor": {"login": "dave"}, "assigner": {"login": "dave"}, "assignee": {"login": "alice"}}]}'
aa assign 14 needs_input --base HEAD >"$sandbox/aa.out" 2>&1 || true
check "an approver already assigned is noted" present "already assigned to #14: alice" "$sandbox/aa.out"
check "and only the others are posted" eq "$(last_stdin)" '{"assignees": ["bob"]}'
out="$(aa release 14 needs_input 2>/dev/null)" || out="(failed)"
check "an approver a person assigned before the bot is not released" eq "$out" "released bob from #14 (needs_input)"
check "alice stays on issue 14" eq "$(assignees 14)" "alice"
before="$(deletes)"
out="$(aa release 12 plan 2>/dev/null)" || out="(failed)"
check "release with nothing of the bot's is a no-op" eq "$out" "nothing to release"
check "and sends no DELETE" eq "$(deletes)" "$before"
seed 15 '{"assignees": ["alice"], "events": [
  {"id": 2, "created_at": "2026-01-02T00:00:00Z", "event": "assigned", "actor": {"login": "dave"}, "assignee": {"login": "alice"}},
  {"id": 1, "created_at": "2026-01-01T00:00:00Z", "event": "assigned", "actor": {"login": "mintswarm-dev[bot]"}, "assignee": {"login": "alice"}}]}'
out="$(aa release 15 plan 2>/dev/null)" || out="(failed)"
check "the latest assigned event is the latest by time, not by position" eq "$out" "nothing to release"
check "alice stays on issue 15" eq "$(assignees 15)" "alice"
seed 16 '{"assignees": ["Alice", "Bob"], "events": [
  {"event": "assigned", "actor": {"login": "dave"}, "assignee": {"login": "Alice"}},
  {"event": "assigned", "actor": {"login": "dave"}, "assignee": {"login": "Bob"}}]}'
before="$(posts)"
out="$(aa assign 16 pr --base HEAD 2>/dev/null)" || out="(failed)"
check "approvers assigned in another case count as assigned" eq "$out" "nothing to assign: #16 is assigned to every approver (pr)"
check "and nothing is posted" eq "$(posts)" "$before"
out="$(aa release 16 pr 2>/dev/null)" || out="(failed)"
check "release matches events without regard to case" eq "$out" "nothing to release"

# --- reclassify.py -----------------------------------------------------------

C="$here/reclassify.py"
check "reclassify.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$C" "$sandbox/reclassify.pyc"
rc() { python3 -I "$C" "$@"; }
writes() { jq -c 'select(.cmd == "gh") | .argv | select(index("PATCH") or index("POST"))' "$argv_log" | wc -l | tr -d ' '; }
issue_type() { jq -r --arg n "$1" '.[$n].type // "null"' "$MSW_STUB_STATE/issues.json"; }
comments() { jq -r --arg n "$1" '.[$n].comments // [] | length' "$MSW_STUB_STATE/issues.json"; }
comment_body() { jq -r --arg n "$1" '.[$n].comments[0].body' "$MSW_STUB_STATE/issues.json"; }
analysis() {  # analysis FILE JSON: an analysis whose ## Split holds JSON
  printf '# Analysis: x\n\n## Components\n\nagents, docs\n\n## Split\n\n```json\n%s\n```\n\n## Approaches\n\n```json\n{"not": "this"}\n```\n' \
    "$2" > "$1"
}
URL=https://github.com/mintswarm/mintswarm/issues/21#artifact-1
FINDING="It spans agents and docs."
analysis "$sandbox/split.md" '{"split": {"proposed": true, "reason": "components", "components": ["agents", "docs"], "finding": "It spans agents and docs."}}'
analysis "$sandbox/nosplit.md" '{"split": {"proposed": false, "reason": "none", "components": ["agents"], "finding": ""}}'
analysis "$sandbox/broken.md" '{"split": {"proposed": true,'
printf '# Analysis: x\n\n## Components\n\nagents\n' > "$sandbox/missing.md"
cat > "$sandbox/decoy.md" <<'MD'
# Analysis: x

## Existing context

The analyse template ends with:

````markdown
## Split

```json
{"split": {"proposed": true, "reason": "other", "components": [], "finding": "decoy"}}
```
````

~~~
## Split
~~~

## Split

```json
{"split": {"proposed": false, "reason": "none", "components": ["agents"], "finding": ""}}
```

## Approaches

```json
{"not": "this"}
```
MD
epic_base="$(commit_config 'tracker: {kinds: {design: {issue_type: Design}, plan: {issue_type: Epic}, implementation: {issue_type: Task}}}')"

seed 21 '{"assignees": [], "events": [], "type": "Task"}'
before="$(writes)"
out="$(rc split 21 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" 2>/dev/null)" || out="(failed)"
check "split re-classifies a Task and prints kind, state, comment and package" eq "$out" "kind: plan
state: Decomposing (re-classified as plan)
comment: https://github.com/mintswarm/mintswarm/issues/21#issuecomment-100
package: Re-classified from Task to Plan: $FINDING (https://github.com/mintswarm/mintswarm/issues/21#issuecomment-100)"
check "split PATCHes the issue through bot-gh.sh" \
  gh_argv_seen '["api","--method","PATCH","repos/{owner}/{repo}/issues/21","--input","-"]'
check "split sends the plan type on stdin" present '{"type": "Plan"}' "$MSW_STUB_STATE/stdin-gh.log"
check "issue 21 is now a Plan" eq "$(issue_type 21)" "Plan"
check "split posts one comment and makes two writes in all" eq "$(comments 21) $(( $(writes) - before ))" "1 2"
check "the comment holds the marker" present "<!-- mintswarm: reclassified -->" <(comment_body 21)
check "the comment names the old and new types" present 'from `Task` to `Plan`' <(comment_body 21)
check "the comment holds the finding" present "$FINDING" <(comment_body 21)
check "the comment holds the analysis URL" present "$URL" <(comment_body 21)
check "the comment names the components" present '`agents`, `docs`' <(comment_body 21)

before="$(writes)"
out="$(rc split 21 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" 2>/dev/null)" || out="(failed)"
check "a re-run on the Plan issue prints the four lines with the existing comment's URL" eq "$out" "kind: plan
state: Decomposing (re-classified as plan)
comment: https://github.com/mintswarm/mintswarm/issues/21#issuecomment-100
package: Re-classified from Task to Plan: $FINDING (https://github.com/mintswarm/mintswarm/issues/21#issuecomment-100)"
check "and sends no PATCH and no second comment" eq "$(( $(writes) - before )) $(comments 21)" "0 1"
check "it lists the issue's comments, all pages" \
  gh_argv_seen '["api","--paginate","--slurp","repos/{owner}/{repo}/issues/21/comments"]'

seed 27 '{"assignees": [], "events": [], "type": "Task", "comment_failures": 1}'
comment_fails() {  # exit 1 when the comment POST fails, after the type has changed
  local rc=0
  rc split 27 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" >"$sandbox/rc.out" 2>&1 || rc=$?
  cat "$sandbox/rc.out"
  eq "$rc" 1
}
check "a failed comment POST fails split with 1" comment_fails
check "and leaves the issue a Plan with no comment" eq "$(issue_type 27) $(comments 27)" "Plan 0"
before="$(writes)"
out="$(rc split 27 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" 2>/dev/null)" || out="(failed)"
check "a re-run posts the missing comment and prints all four lines" eq "$out" "kind: plan
state: Decomposing (re-classified as plan)
comment: https://github.com/mintswarm/mintswarm/issues/27#issuecomment-100
package: Re-classified from Task to Plan: $FINDING (https://github.com/mintswarm/mintswarm/issues/27#issuecomment-100)"
check "with exactly one comment, one write and no PATCH" eq "$(comments 27) $(( $(writes) - before ))" "1 1"
check "the late comment holds the marker and names Task to Plan" \
  present 'from `Task` to `Plan`' <(comment_body 27)
check "and the marker" present "<!-- mintswarm: reclassified -->" <(comment_body 27)
before="$(writes)"
out="$(rc split 27 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" 2>/dev/null)" || out="(failed)"
check "a further re-run writes nothing and reuses the comment" \
  eq "$(comments 27) $(( $(writes) - before )) $(sed -n 3p <<<"$out")" \
  "1 0 comment: https://github.com/mintswarm/mintswarm/issues/27#issuecomment-100"

seed 22 '{"assignees": [], "events": [], "type": "Task"}'
before="$(writes)"
out="$(rc split 22 --analysis "$sandbox/nosplit.md" --analysis-url "$URL" --base "$real_base" 2>/dev/null)" || out="(failed)"
check "no proposed split prints kind: implementation" eq "$out" "kind: implementation"
check "and writes nothing" eq "$(( $(writes) - before ))" "0"
check "issue 22 stays a Task" eq "$(issue_type 22)" "Task"

out="$(rc split 22 --analysis "$sandbox/decoy.md" --analysis-url "$URL" --base "$real_base" 2>&1)" || out="(failed) $out"
check "a fenced ## Split decoy is ignored" eq "$out" "kind: implementation"
check "a missing ## Split section exits 2" exits_2 rc split 22 --analysis "$sandbox/missing.md" --analysis-url "$URL" --base "$real_base"
check "broken json in ## Split exits 2" exits_2 rc split 22 --analysis "$sandbox/broken.md" --analysis-url "$URL" --base "$real_base"
check "an analysis URL not on github.com exits 2" \
  exits_2 rc split 22 --analysis "$sandbox/split.md" --analysis-url https://example.com/a --base "$real_base"
seed 23 '{"assignees": [], "events": [], "type": "Design"}'
before="$(writes)"
check "a Design issue with a proposed split exits 2" \
  exits_2 rc split 23 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base"
check "and nothing is written for any of them" eq "$(( $(writes) - before )) $(issue_type 22) $(issue_type 23)" "0 Task Design"

seed 24 '{"assignees": [], "events": [], "type": "Task", "untypeable": true}'
split_untypeable() {  # exit 1 when GitHub does not take the type, with no comment
  local rc=0
  rc split 24 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" >"$sandbox/rc.out" 2>&1 || rc=$?
  cat "$sandbox/rc.out"
  eq "$rc" 1 && grep -qF "did not set #24's type to Plan" "$sandbox/rc.out"
}
check "an issue GitHub does not re-type fails split with 1" split_untypeable
check "and gets no comment" eq "$(comments 24)" "0"

seed 25 '{"assignees": [], "events": [], "type": "Task"}'
before="$(writes)"
out="$(rc split 25 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$real_base" --dry-run 2>/dev/null)" || out="(failed)"
check "split --dry-run prints kind, state and package" eq "$out" "kind: plan
state: Decomposing (re-classified as plan)
package: Re-classified from Task to Plan: $FINDING"
check "and writes nothing" eq "$(( $(writes) - before )) $(issue_type 25) $(comments 25)" "0 Task 0"

seed 26 '{"assignees": [], "events": [], "type": "Task"}'
rc split 26 --analysis "$sandbox/split.md" --analysis-url "$URL" --base "$epic_base" >/dev/null 2>&1 || true
check "the plan type comes from the config at the base and goes into the PATCH body" \
  present '{"type": "Epic"}' "$MSW_STUB_STATE/stdin-gh.log"
check "and into the comment" present 'from `Task` to `Epic`' <(comment_body 26)
check "issue 26 is now an Epic" eq "$(issue_type 26)" "Epic"

seed 31 '{"assignees": [], "events": [], "type": "Plan"}'
seed 32 '{"assignees": [], "events": [], "type": "Design"}'
seed 33 '{"assignees": [], "events": [], "type": "Task"}'
seed 34 '{"assignees": [], "events": []}'
check "check-parent accepts a Plan parent" rc check-parent 31 --base "$real_base"
check "check-parent accepts a Design parent" rc check-parent 32 --base "$real_base"
check "check-parent refuses a Task parent with 1" fails_saying \
  "refusing to file children under #33: type Task is neither Plan nor Design" rc check-parent 33 --base "$real_base"
check "check-parent refuses an untyped parent" fails_saying \
  "refusing to file children under #34: type none is neither Plan nor Design" rc check-parent 34 --base "$real_base"
refused_with_1() { local rc=0; rc check-parent 33 --base "$real_base" >/dev/null 2>&1 || rc=$?; eq "$rc" 1; }
check "the refusal exits 1" refused_with_1
check "check-parent with a bad number exits 2" exits_2 rc check-parent 0 --base "$real_base"

# --- land.py -----------------------------------------------------------------

L="$here/land.py"
check "land.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$L" "$sandbox/land.pyc"
sha_of() { printf '%040x' "$1"; }
OTHER_SHA="$(sha_of 4095)"
for f in pulls commits threads issues merges reviews review-comments; do
  [ -f "$MSW_STUB_STATE/$f.json" ] || printf '{}\n' > "$MSW_STUB_STATE/$f.json"
done
stub_update() {  # stub_update FILE JQ ARGS...: rewrite FILE.json in the gh stub's state with JQ
  local f="$MSW_STUB_STATE/$1.json" filter="$2"; shift 2
  jq "$@" "$filter" "$f" > "$f.new" && mv "$f.new" "$f"
}
fixture() {  # fixture N [PULL_JQ [COMMIT_JQ [THREADS_JSON [COMMENTS_JQ]]]]: PR N, ready to land at $(sha_of N)
  local n="$1" sha
  sha="$(sha_of "$1")"
  stub_update pulls ".[\$n] = ([{number: (\$n | tonumber), title: \"feat(agents): land \(\$n)\", state: \"open\",
      draft: false, base: {ref: \"master\"}, labels: [{name: \"mintswarm\"}],
      head: {ref: \"mintswarm/\(\$n)-land\", sha: \$sha, repo: {full_name: \"mintswarm/mintswarm\"}},
      mergeable: true, mergeable_state: \"blocked\", merged: false}] | ${2:-.})" --arg n "$n" --arg sha "$sha"
  stub_update commits ".[\$sha] = ({check_runs: ([\"gates-matrix\", \"gates (ubuntu)\", \"gates (macos)\"]
        | map({name: ., status: \"completed\", conclusion: \"success\", app: {slug: \"github-actions\"}, check_suite: {id: 7}})),
      check_suites: [
        {id: 7, app: {slug: \"github-actions\"}, status: \"completed\", conclusion: \"success\", latest_check_runs_count: 3},
        {id: 8, app: {slug: \"some-app\"}, status: \"queued\", conclusion: null, latest_check_runs_count: 0},
        {id: 9, app: {slug: \"github-actions\"}, status: \"completed\", conclusion: \"skipped\", latest_check_runs_count: 0}],
      statuses: [{context: \"ci/legacy\", state: \"success\"}]} | ${3:-.})" --arg sha "$sha"
  stub_update threads '.[$n] = $t' --arg n "$n" --argjson t "${4:-[true, true, true]}"
  stub_update issues ".[\$n] = {assignees: [], events: [], unassignable: [], type: null,
      comments: ([{id: 500, created_at: \"2026-11-23T12:00:00Z\", user: {login: \"mintswarm-dev[bot]\"}, html_url: \"https://github.com/x\",
        body: \"<!-- mintswarm: steer -->\n<!-- mintswarm: steer approval -->\nApproved head: \(\$sha)\nTime: 2026-11-23T12:00:00Z\n\nThe maintainer approved this PR in the session:\n\n> I approve PR #\(\$n)\n\"}] | ${5:-.})}" \
    --arg n "$n" --arg sha "$sha"
}
land() { env MSW_LAND_RETRY_DELAY=0 "$L" "$@"; }
merge_calls() { jq --arg n "$1" '[.calls // [] | .[] | select(.pr == $n)] | length' "$MSW_STUB_STATE/merges.json"; }
mints() { jq -r 'select(.cmd == "curl") | .argv | select(index("https://api.github.com/app/installations/4242/access_tokens")) | "x"' "$argv_log" | wc -l | tr -d ' '; }
refused_land() {  # refused_land TEXT N [ARGS...]: land.py N --sha $(sha_of N) exits 1 saying TEXT, with no merge call for N
  local text="$1" n="$2" rc=0; shift 2
  land "$n" --sha "$(sha_of "$n")" "$@" >"$sandbox/land.out" 2>&1 || rc=$?
  cat "$sandbox/land.out"
  eq "$rc" 1 || return 1
  grep -qF -- "$text" "$sandbox/land.out" || { echo "no line: $text"; return 1; }
  eq "$(merge_calls "$n")" 0
}

# A PR that passes every check is merged as the steward, at the approved head.
fixture 90
land_argv_start="$(wc -l < "$argv_log" | tr -d ' ')"
land_gh_start="$(gh_calls)"
land_mints_before="$(mints)"
land_merged() {
  env MSW_TOKEN="$DUMMY" MSW_LAND_RETRY_DELAY=0 "$L" 90 --sha "$(sha_of 90)" > "$sandbox/land90.out" 2>&1 || { cat "$sandbox/land90.out"; return 1; }
  cat "$sandbox/land90.out"
  grep -qxF "merged #90 at $(sha_of 90)" "$sandbox/land90.out" || return 1
  grep -qxF "merge_commit_sha: $(printf 'f%.0s' $(seq 40))" "$sandbox/land90.out" || return 1
  grep -qxF "merged_by: mintswarm-dev-steward[bot]" "$sandbox/land90.out"
}
check "land.py merges a PR that passes every check and prints the merge commit and merged_by" land_merged
check "with one PUT to pulls/90/merge" eq "$(merge_calls 90)" 1
check "whose body is squash at the approved sha, titled with the PR title and number" \
  eq "$(jq -cS '.calls[-1] | [.method, .body]' "$MSW_STUB_STATE/merges.json")" \
     "$(jq -cnS --arg sha "$(sha_of 90)" '["PUT", {merge_method: "squash", sha: $sha, commit_title: "feat(agents): land 90 (#90)"}]')"
check "it read pulls/90 three times (settled, again right before the PUT, after it)" \
  eq "$(jq -r '.["90"]' "$MSW_STUB_STATE/pulls-reads.json")" 3
check "it asked for the commit's check suites" \
  sh -c 'jq -e "select(.cmd == \"gh\") | .argv | select(index(\"--paginate\") and index(\"--slurp\")) | .[-1] | select(test(\"commits/$2/check-suites\"))" "$1" >/dev/null' \
  sh "$argv_log" "$(sha_of 90)"
check "exactly one access token was minted" eq "$(( $(mints) - land_mints_before ))" 1
land_argv() { tail -n "+$((land_argv_start + 1))" "$argv_log"; }
check "it asked for contents and pull_requests write, checks, statuses and metadata read" \
  eq "$(land_argv | jq -r 'select(.cmd == "curl") | .argv | index("-d") as $i | if $i then .[$i + 1] else empty end' | jq -cS .permissions)" \
     '{"checks":"read","contents":"write","metadata":"read","pull_requests":"write","statuses":"read"}'
check "it read the steward's key, and only from the recorded keychain store, with no other security call" \
  eq "$(land_argv | jq -c 'select(.cmd == "security") | .argv' | sort -u)" \
     "[\"find-generic-password\",\"-s\",\"mintswarm-dev-steward-private-key\",\"-a\",\"$CLIENT_ID\",\"-w\"]"
land_gh_env() { tail -n "+$((land_gh_start + 1))" "$MSW_STUB_STATE/gh-env.log"; }
check "every gh call ran with the minted steward token, not the caller's MSW_TOKEN" \
  eq "$(land_gh_env | jq -r '[.GH_TOKEN == $t, .MSW_TOKEN == null] | @text' --arg t "$MINTED" | sort -u)" "[true,true]"
check "the token is in no argv" absent "$MINTED" "$argv_log"
check "and not in land.py's output" absent "$MINTED" "$sandbox/land90.out"
check "the check runs and review threads were read over more than one page" \
  sh -c 'jq -e "select(.cmd == \"gh\") | .argv | select(index(\"--paginate\") and index(\"--slurp\")) | .[-1] | select(test(\"check-runs[?]filter=latest\"))" "$1" >/dev/null \
         && jq -e "select(.cmd == \"gh\") | .argv | select(index(\"after=2\"))" "$1" >/dev/null' sh "$argv_log"

fixture 91
land_dry() {
  land 91 --sha "$(sha_of 91)" --dry-run > "$sandbox/land91.out" 2>&1 || { cat "$sandbox/land91.out"; return 1; }
  grep -qF "passes every check; nothing merged (--dry-run)" "$sandbox/land91.out"
}
check "--dry-run passes every check" land_dry
check "and sends no PUT" eq "$(merge_calls 91)" 0
fixture 113 '[(.[0] | .mergeable = null), (.[0] | .mergeable = null), .[0]]'
check "a mergeable that GitHub works out on the third read passes (--dry-run)" land 113 --sha "$(sha_of 113)" --dry-run

# Each refusal merges nothing.
fixture 92 ".[0].head.sha = \"$OTHER_SHA\""
check "refused: the head moved since the approval" refused_land "it moved since the approval" 92
fixture 93 . '.check_runs[1].conclusion = "failure"'
check "refused: a failing check run" refused_land "the check run gates (ubuntu) concluded failure, not success" 93
fixture 94 . '.check_runs[2] |= (.status = "in_progress" | .conclusion = null)'
check "refused: a pending check run" refused_land "the check run gates (macos) is in_progress, not completed" 94
fixture 95 . '.statuses[0].state = "failure"'
check "refused: a failing status" refused_land "the status ci/legacy is failure, not success" 95
fixture 96 . '.statuses[0].state = "pending"'
check "refused: a pending status" refused_land "the status ci/legacy is pending, not success" 96
fixture 114 . '.status_total = 101'
check "refused: more statuses than one page holds" refused_land "more than one page of 100" 114
fixture 97 . '.check_runs |= map(select(.name != "gates-matrix"))'
check "refused: no gates-matrix check run" refused_land "no gates-matrix check run by github-actions" 97
fixture 117 . '.check_runs |= map(select(.name == "gates-matrix")) | .check_suites[0] |= (.status = "in_progress" | .conclusion = null | .latest_check_runs_count = 1)'
check "refused: only gates-matrix has run, the gate runs not created yet" \
  refused_land "the check suite 7 of github-actions is in_progress, not completed" 117
check "which also says the gates have not run" present "has no gate check run yet: the gates have not run" "$sandbox/land.out"
fixture 118 . '.check_runs |= map(select(.name == "gates-matrix")) | .check_suites[0].latest_check_runs_count = 1'
check "refused: gates-matrix alone, even in a completed suite" refused_land "has no gate check run yet: the gates have not run" 118
fixture 119 . '.check_suites[0] |= (.status = "completed" | .conclusion = "failure")'
check "refused: a check suite that concluded failure" refused_land "the check suite 7 of github-actions concluded failure, not success" 119
fixture 120 . '.check_suites |= map(select(.id != 7))'
check "refused: the gates-matrix run's check suite is missing" refused_land "the check suite 7 of gates-matrix is not among the check suites" 120
fixture 140 . '.check_runs |= map(del(.check_suite)) | .check_runs[1].app.slug = "impostor"'
check "refused: a gates-matrix run with no check suite" refused_land "the check suite None of gates-matrix is not among" 140
check "with no gate lookup among runs that have no suite either" absent "in check suite None" "$sandbox/land.out"
fixture 121 . '.check_suites += [{id: 10, app: {slug: "other-ci"}, status: "in_progress", conclusion: null, latest_check_runs_count: 1}]'
check "refused: another App's check suite still running" refused_land "the check suite 10 of other-ci is in_progress, not completed" 121
fixture 122 . '.check_runs[0].app.slug = "impostor"'
check "refused: a gates-matrix run created by another App" refused_land "no gates-matrix check run by github-actions" 122
fixture 123 . '.check_runs[1].app.slug = "impostor"'
check "refused: a gate run in the gates suite created by another App" \
  refused_land "the check run gates (ubuntu) in check suite 7 is by 'impostor', not github-actions" 123
fixture 124 . '.check_runs[2].conclusion = "skipped"'
check "refused: a skipped check run (only success passes)" refused_land "the check run gates (macos) concluded skipped, not success" 124
fixture 125 . '.check_runs[2].conclusion = "neutral"'
check "refused: a neutral check run" refused_land "the check run gates (macos) concluded neutral, not success" 125
fixture 98 . . '[true, true, false]'
check "refused: an unresolved review thread, on the second page" refused_land "1 review thread(s) unresolved" 98
fixture 99 '.[0].mergeable_state = "dirty"'
check "refused: the PR conflicts" refused_land "the PR conflicts with master" 99
fixture 100 '.[0].mergeable = null'
check "refused: mergeable is still null after the retries" refused_land "has not worked out whether the PR is mergeable" 100
check "after reading pulls/100 six times" eq "$(jq -r '.["100"]' "$MSW_STUB_STATE/pulls-reads.json")" 6
fixture 101 '.[0].draft = true'
check "refused: a draft" refused_land "the PR is a draft" 101
fixture 102 '.[0].state = "closed"'
check "refused: a closed PR" refused_land "the PR is closed, not open" 102
fixture 103 '.[0].base.ref = "release"'
check "refused: a base other than master" refused_land "the PR targets 'release', not master" 103
fixture 104 '.[0].head.ref = "feat/104-land"'
check "refused: a head that is not a mintswarm/* branch" refused_land "is not a mintswarm/* branch" 104
fixture 105 '.[0].head.repo.full_name = "someone/mintswarm"'
check "refused: a head in a fork" refused_land "the head branch is in 'someone/mintswarm', not mintswarm/mintswarm" 105
fixture 106 '.[0].labels += [{name: "hold"}]'
check "refused: the hold label" refused_land "the PR carries the hold label" 106
fixture 107 . . '[true, true, true]' '[]'
check "refused: no approval steer" refused_land "no approval steer by mintswarm-dev[bot] names the head" 107
fixture 108 . . '[true, true, true]' ".[0].body |= sub(\"Approved head: [0-9a-f]+\"; \"Approved head: $OTHER_SHA\")"
check "refused: a steer naming another sha" refused_land "no approval steer by mintswarm-dev[bot] names the head" 108
fixture 109 . . '[true, true, true]' '.[0].user.login = "zmij"'
check "refused: a steer by another author" refused_land "no approval steer by mintswarm-dev[bot] names the head" 109
fixture 115 . . '[true, true, true]' '.[0].body |= sub("<!-- mintswarm: steer -->"; "<!-- mintswarm: steer --> approved")'
check "refused: a steer without the exact marker line" refused_land "no approval steer by mintswarm-dev[bot] names the head" 115
fixture 126 . . '[true, true, true]' '.[0].body |= sub("<!-- mintswarm: steer approval -->\n"; "")'
check "refused: a steer without the approval marker line" refused_land "no approval steer by mintswarm-dev[bot] names the head" 126
fixture 127 . . '[true, true, true]' '.[0].body |= "Round 2 summary, relaying the stage:\n\n" + . + "\nEnd of the stage output.\n"'
check "refused: the approval lines embedded in a longer bot comment" refused_land "no approval steer by mintswarm-dev[bot] names the head" 127
fixture 128 . . '[true, true, true]' '.[0].body |= sub("Approved head: "; "Time: 2026-11-23T12:00:00Z\nApproved head: ")'
check "refused: the approved-head line not third" refused_land "no approval steer by mintswarm-dev[bot] names the head" 128
fixture 129 . . '[true, true, true]' '. + [{id: 501, created_at: "2026-11-23T12:05:00Z", user: {login: "mintswarm-dev[bot]"},
  body: "<!-- mintswarm: steer -->\nThe maintainer asked to wait for the rebase first.\n"}]'
check "refused: a later steer withdraws the approval" refused_land "a later steer by mintswarm-dev[bot] withdraws the approval" 129
fixture 130 . . '[true, true, true]' '[{id: 499, created_at: "2026-11-23T11:00:00Z", user: {login: "mintswarm-dev[bot]"},
  body: "<!-- mintswarm: steer -->\nThe maintainer asked for a rebase.\n"}] + . + [{id: 502, created_at: "2026-11-23T12:10:00Z",
  user: {login: "mintswarm-dev[bot]"}, body: "Round summary.\n\n<!-- mintswarm: steer -->\n"}]'
check "refused: a later bot comment with the steer marker on a later line withdraws the approval" \
  refused_land "a later steer by mintswarm-dev[bot] withdraws the approval" 130
fixture 136 . . '[true, true, true]' '. + [{id: 504, created_at: "2026-11-23T12:05:00Z", user: {login: "zmij", type: "User"},
  body: "wait"}]'
check "refused: a later comment by a person withdraws the approval" refused_land "a later steer by zmij withdraws the approval" 136
fixture 137 . . '[true, true, true]' '[{id: 499, created_at: "2026-11-23T11:00:00Z", user: {login: "mintswarm-dev[bot]"},
  body: "<!-- mintswarm: steer -->\nThe maintainer asked for a rebase.\n"}] + . + [
  {id: 505, created_at: "2026-11-23T12:10:00Z", user: {login: "mintswarm-dev[bot]"}, body: "Round summary.\n"},
  {id: 506, created_at: "2026-11-23T12:11:00Z", user: {login: "github-actions[bot]"}, body: "<!-- mintswarm: steer -->\n"},
  {id: 507, created_at: "2026-11-23T12:12:00Z", user: {login: "some-app", type: "Bot"}, body: "<!-- mintswarm: steer -->\n"}]'
check "an approval after an earlier steer, followed by bot comments that are not steers, passes (--dry-run)" \
  land 137 --sha "$(sha_of 137)" --dry-run
fixture 138 . . '[true, true, true]' '. + [{id: 508, created_at: "2026-11-23T12:05:00Z", user: {login: "mintswarm-dev[bot]"},
  body: "Relayed decision:\n  <!-- mintswarm: steer -->  \nHold this one.\n"}]'
check "refused: a later steer whose marker line has surrounding whitespace withdraws the approval" \
  refused_land "a later steer by mintswarm-dev[bot] withdraws the approval" 138
fixture 131 . . '[true, true, true]' '[{id: 503, created_at: "2026-11-23T13:00:00Z", user: {login: "mintswarm-dev[bot]"},
  body: "<!-- mintswarm: steer -->\nThe maintainer asked to wait.\n"}] + .'
check "refused: the latest steer by created_at wins, whatever the comment order" \
  refused_land "a later steer by mintswarm-dev[bot] withdraws the approval" 131

fixture 141
stub_update reviews '.["141"] = [{id: 9001, user: {login: "zmij", type: "User"}, state: "CHANGES_REQUESTED",
  submitted_at: "2026-11-23T12:05:00Z", body: "Not like this."}]'
check "refused: a later review by a person requesting changes withdraws the approval" \
  refused_land "a later review by zmij (CHANGES_REQUESTED) withdraws the approval" 141
fixture 142
stub_update reviews '.["142"] = [{id: 9002, user: {login: "zmij", type: "User"}, state: "COMMENTED",
  submitted_at: "2026-11-23T12:05:00Z", body: "Hold on."}]'
check "refused: a later comment-only review by a person withdraws the approval" \
  refused_land "a later review by zmij (COMMENTED) withdraws the approval" 142
fixture 143
stub_update reviews '.["143"] = [
  {id: 9003, user: {login: "zmij", type: "User"}, state: "APPROVED", submitted_at: "2026-11-23T11:00:00Z", body: ""},
  {id: 9004, user: {login: "zmij", type: "User"}, state: "PENDING", body: "draft"},
  {id: 9005, user: {login: "mintswarm-dev[bot]", type: "Bot"}, state: "COMMENTED", submitted_at: "2026-11-23T12:05:00Z",
    body: "Review round summary."},
  {id: 9006, user: {login: "copilot[bot]", type: "Bot"}, state: "COMMENTED", submitted_at: "2026-11-23T12:06:00Z",
    body: "<!-- mintswarm: steer -->"}]'
check "an earlier review, a pending review and later bot reviews that are not steers pass (--dry-run)" \
  land 143 --sha "$(sha_of 143)" --dry-run
fixture 144 . . '[true, true, true]' '[{id: 498, created_at: "2026-11-23T11:00:00Z", updated_at: "2026-11-23T12:05:00Z",
  user: {login: "zmij", type: "User"}, body: "wait"}] + .'
check "refused: an earlier comment edited after the approval withdraws it" \
  refused_land "a steer by zmij edited after the approval withdraws the approval" 144
fixture 145 . . '[true, true, true]' '[{id: 497, created_at: "2026-11-23T11:00:00Z", updated_at: "2026-11-23T11:30:00Z",
  user: {login: "zmij", type: "User"}, body: "looks good"}] + .'
check "an earlier comment edited before the approval passes (--dry-run)" land 145 --sha "$(sha_of 145)" --dry-run
fixture 147
stub_update review-comments '.["147"] = [{id: 9101, user: {login: "zmij", type: "User"}, created_at: "2026-11-23T11:00:00Z",
  updated_at: "2026-11-23T12:05:00Z", body: "Not this way."}]'
check "refused: an earlier inline review comment edited after the approval withdraws it" \
  refused_land "an inline review comment by zmij edited after the approval withdraws the approval" 147
check "having read pulls/147/comments over every page" \
  sh -c 'jq -e "select(.cmd == \"gh\") | .argv | select(index(\"--paginate\") and index(\"--slurp\")) | .[-1] | select(test(\"pulls/147/comments\"))" "$1" >/dev/null' \
  sh "$argv_log"
fixture 148
stub_update review-comments '.["148"] = [{id: 9102, user: {login: "zmij", type: "User"}, created_at: "2026-11-23T12:05:00Z",
  updated_at: "2026-11-23T12:05:00Z", body: "One more thing."}]'
check "refused: a later inline review comment withdraws the approval" \
  refused_land "a later inline review comment by zmij withdraws the approval" 148
fixture 149
stub_update review-comments '.["149"] = [
  {id: 9103, user: {login: "zmij", type: "User"}, created_at: "2026-11-23T11:00:00Z", updated_at: "2026-11-23T11:30:00Z", body: "nit"},
  {id: 9104, user: {login: "copilot[bot]", type: "Bot"}, created_at: "2026-11-23T12:05:00Z", updated_at: "2026-11-23T12:06:00Z",
    body: "<!-- mintswarm: steer -->"}]'
check "an inline comment edited before the approval and a later bot one that is not a steer pass (--dry-run)" \
  land 149 --sha "$(sha_of 149)" --dry-run

# Right before the merge, pulls/N and the threads are read again.
fixture 132 '[.[0], (.[0] | .labels += [{name: "hold"}])]'
check "refused: the hold label added during the checks" refused_land "the PR carries the hold label" 132
check "which says the labels changed" present "the PR's labels changed during the checks" "$sandbox/land.out"
fixture 133 . . '{"reads": [[true, true, true], [true, true, false]]}'
check "refused: a review thread opened during the checks" refused_land "1 review thread(s) unresolved" 133
fixture 134 '[.[0], (.[0] | .mergeable_state = "dirty")]'
check "refused: a conflict that appeared during the checks" refused_land "the PR conflicts with master" 134
fixture 135 '[.[0], (.[0] | .title = "feat(agents): something else")]'
check "refused: the title changed during the checks" refused_land "the PR's title changed during the checks" 135
fixture 139
stub_update issues '.["139"].comment_reads = [.["139"].comments, (.["139"].comments + [{id: 509, created_at: "2026-11-23T12:05:00Z",
  user: {login: "zmij", type: "User"}, body: "wait, not yet"}])]'
check "refused: a steer posted during the checks withdraws the approval" \
  refused_land "a later steer by zmij withdraws the approval" 139
check "after reading the comments twice" eq "$(jq -r '.["139"]' "$MSW_STUB_STATE/comments-reads.json")" 2
fixture 146
stub_update reviews '.["146"] = {reads: [[], [{id: 9007, user: {login: "zmij", type: "User"}, state: "CHANGES_REQUESTED",
  submitted_at: "2026-11-23T12:05:00Z", body: "Stop."}]]}'
check "refused: a review submitted during the checks withdraws the approval" \
  refused_land "a later review by zmij (CHANGES_REQUESTED) withdraws the approval" 146
check "after reading the reviews twice" eq "$(jq -r '.["146"]' "$MSW_STUB_STATE/reviews-reads.json")" 2
fixture 150
stub_update review-comments '.["150"] = {reads: [[], [{id: 9105, user: {login: "zmij", type: "User"},
  created_at: "2026-11-23T12:05:00Z", updated_at: "2026-11-23T12:05:00Z", body: "Stop."}]]}'
check "refused: an inline review comment posted during the checks withdraws the approval" \
  refused_land "a later inline review comment by zmij withdraws the approval" 150
check "after reading the inline review comments twice" \
  eq "$(jq -r '.["150"]' "$MSW_STUB_STATE/review-comments-reads.json")" 2
fixture 151 . '{reads: [., (.check_runs[1] |= (.status = "in_progress" | .conclusion = null))]}'
check "refused: a check run re-run during the checks" refused_land "the check run gates (ubuntu) is in_progress, not completed" 151
check "after reading the check runs twice" \
  eq "$(jq -r --arg k "$(sha_of 151)/check-runs" '.[$k]' "$MSW_STUB_STATE/commits-reads.json")" 2
fixture 152 . '{reads: [., (.check_suites[0] |= (.status = "in_progress" | .conclusion = null))]}'
check "refused: a check suite re-run during the checks" \
  refused_land "the check suite 7 of github-actions is in_progress, not completed" 152
fixture 153 . '{reads: [., (.statuses[0].state = "failure")]}'
check "refused: a status that failed during the checks" refused_land "the status ci/legacy is failure, not success" 153
check "after reading the statuses twice" \
  eq "$(jq -r --arg k "$(sha_of 153)/status" '.[$k]' "$MSW_STUB_STATE/commits-reads.json")" 2
fixture 116 '.[0].mergeable = false | .[0].mergeable_state = "unknown"'
check "refused: GitHub reports the PR not mergeable" refused_land "GitHub reports the PR not mergeable" 116

check "a refusal says the PR was not merged" present "#116 was not merged" "$sandbox/land.out"

# Only the steward: a config naming mintswarm-dev or its key is refused before anything is minted.
with_cfg() { local cfg="$1"; shift; ( export MSW_STEWARD_CONFIG="$cfg"; "$@" ); }
fixture 110
mints_before="$(mints)"
check "refused: a steward config that is mintswarm-dev's" \
  with_cfg "$MSW_APP_CONFIG" refused_land "names the App 'mintswarm-dev', not mintswarm-dev-steward" 110
jq '.keychain_service = "mintswarm-dev-private-key"' "$steward_cfg" > "$sandbox/steward-dev-service.json"
check "refused: a steward config naming mintswarm-dev's key service" \
  with_cfg "$sandbox/steward-dev-service.json" refused_land "not mintswarm-dev-steward-private-key" 110
check "refused: no steward config" with_cfg "$sandbox/no-steward.json" refused_land "no steward config at" 110
check "and none of them minted a token" eq "$(mints)" "$mints_before"
check "a sha that is not 40 lowercase hex characters is bad usage" exits_2 land 110 --sha "$(sha_of 110 | tr a-f A-F)A"
check "a short sha is bad usage" exits_2 land 110 --sha abc1234
check "a PR number of 0 is bad usage" exits_2 land 0 --sha "$(sha_of 110)"
check "no --sha is bad usage" exits_2 land 110
check "and none of them minted a token either" eq "$(mints)" "$mints_before"

# The kill switch: without the bypass GitHub refuses the merge, and land.py exits 1.
fixture 112
stub_update merges '.merge_refused = true'
kill_switch() {
  local rc=0
  land 112 --sha "$(sha_of 112)" > "$sandbox/land.out" 2>&1 || rc=$?
  cat "$sandbox/land.out"
  eq "$rc" 1 && grep -qF "nothing was merged" "$sandbox/land.out"
}
check "refused merge call (the bypass removed): land.py exits 1, saying nothing was merged" kill_switch
check "after one merge call, and #112 is not merged" \
  eq "$(merge_calls 112) $(jq -r '.merged // [] | index("112")' "$MSW_STUB_STATE/merges.json")" "1 null"
stub_update merges 'del(.merge_refused)'
check "TMPDIR is empty after land.py" tmp_empty

# --- msw-state.py ------------------------------------------------------------

MS="$here/msw-state.py"
check "msw-state.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$MS" "$sandbox/msw-state.pyc"
ms() { python3 -I "$MS" "$@"; }
field_posts() { jq -c 'select(.cmd == "gh") | .argv | select(index("POST")) | select(any(.[]; endswith("/issue-field-values")))' "$argv_log" | wc -l | tr -d ' '; }
label_writes() { jq -c 'select(.cmd == "gh") | .argv | select(index("POST") or index("PUT")) | select(any(.[]; endswith("/labels")))' "$argv_log" | wc -l | tr -d ' '; }
graphql_calls() { jq -c 'select(.cmd == "gh") | .argv | select(.[1] == "graphql")' "$argv_log" | wc -l | tr -d ' '; }
mutations() { grep -c setIssueStatus "$MSW_STUB_STATE/stdin-gh.log" || true; }
field_of() { jq -r --arg n "$1" '[.[$n].fields // [] | .[] | select(.issue_field_id == 50325307) | .value] | join(",")' "$MSW_STUB_STATE/issues.json"; }
status_of() { jq -r --arg n "$1" '.[$n].status // "unset"' "$MSW_STUB_STATE/issues.json"; }
labels_of() { jq -r --arg n "$1" '.[$n].labels // [] | join(",")' "$MSW_STUB_STATE/issues.json"; }
repo_label_count() { jq -r --arg l "$1" '[.[] | select(.name == $l)] | length' "$MSW_STUB_STATE/labels.json"; }
ms_rc() {  # ms_rc RC ARGS...: msw-state.py exits RC
  local want="$1" rc=0; shift
  ms "$@" >"$sandbox/ms.out" 2>&1 || rc=$?
  cat "$sandbox/ms.out"
  eq "$rc" "$want"
}
state_cfg() {  # state_cfg SOURCE STATUS: a config with tracker.state only
  printf 'tracker: {state: {source: %s, field: msw-state, field_id: 50325307, label: "msw-state:{value}", status: %s}}' "$1" "$2"
}
bad_source_base="$(commit_config "$(state_cfg other issue_status)")"
bad_status_base="$(commit_config "$(state_cfg field on)")"
bad_id_base="$(commit_config 'tracker: {state: {source: field, field: msw-state, field_id: "50325307", label: "msw-state:{value}", status: off}}')"
bad_label_base="$(commit_config 'tracker: {state: {source: labels, field: msw-state, field_id: 50325307, label: "msw-state", status: off}}')"
off_base="$(commit_config "$(state_cfg field off)")"
labels_only_base="$(commit_config 'tracker: {state: {source: labels, label: "msw-state:{value}", status: off}}')"
labels_base="$(commit_config "$(state_cfg labels off)")"
labels_status_base="$(commit_config "$(state_cfg labels issue_status)")"

check "an unknown value exits 2" exits_2 ms set 41 bogus --base "$real_base"
check "a source other than field or labels exits 2" exits_2 ms set 41 queued --base "$bad_source_base"
check "a status other than issue_status or off exits 2" exits_2 ms set 41 queued --base "$bad_status_base"
check "a field_id that is not an integer exits 2" exits_2 ms set 41 queued --base "$bad_id_base"
check "a label without {value} exits 2" exits_2 ms set 41 queued --base "$bad_label_base"
check "number 0 exits 2" exits_2 ms set 0 queued --base "$real_base"

seed 41 '{"status": "Todo"}'
posts_before="$(field_posts)" mut_before="$(mutations)"
out="$(ms set 41 implementing --base "$real_base" 2>&1)" || out="(failed) $out"
check "set on an unset issue sets the field and the mapped status" eq "$out" "set #41 msw-state to implementing (was none)
set #41 status to In progress (was Todo)"
check "it POSTs the issue's field values through bot-gh.sh" \
  gh_argv_seen '["api","--method","POST","repos/{owner}/{repo}/issues/41/issue-field-values","--input","-"]'
check "with exactly the field id and the value on stdin" \
  present '{"issue_field_values": [{"field_id": 50325307, "value": "implementing"}]}' "$MSW_STUB_STATE/stdin-gh.log"
check "one field POST and one setIssueStatus with the In progress id" \
  eq "$(( $(field_posts) - posts_before )) $(( $(mutations) - mut_before )) $(tail -n 2 "$MSW_STUB_STATE/stdin-gh.log" | head -n 1 | jq -r .variables.status)" \
  "1 1 S_progress"
check "issue 41 now has implementing and In progress" eq "$(field_of 41) $(status_of 41)" "implementing In progress"
out="$(ms get 41 --base "$real_base" 2>&1)" || out="(failed) $out"
check "get prints the value and the status" eq "$out" "msw-state: implementing
status: In progress"

posts_before="$(field_posts)" mut_before="$(mutations)"
out="$(ms set 41 implementing --base "$real_base" 2>&1)" || out="(failed) $out"
check "the same value again writes nothing" eq "$out $(( $(field_posts) - posts_before )) $(( $(mutations) - mut_before ))" \
  "#41 msw-state is already implementing
#41 status is already In progress 0 0"
seed 41 "$(jq -c '.["41"] | .status = "In review"' "$MSW_STUB_STATE/issues.json")"
out="$(ms set 41 implementing --base "$real_base" 2>&1)" || out="(failed) $out"
check "a status moved by GitHub is reset with no field POST" \
  eq "$out $(( $(field_posts) - posts_before )) $(status_of 41)" "#41 msw-state is already implementing
set #41 status to In progress (was In review) 0 In progress"

seed 47 '{"status": "In progress"}'
mut_before="$(mutations)"
out="$(ms set 47 analysing --base "$real_base" 2>&1)" || out="(failed) $out"
check "a status that already matches makes no mutation" eq "$out $(( $(mutations) - mut_before ))" \
  "set #47 msw-state to analysing (was none)
#47 status is already In progress 0"

seed 48 '{"status": "Todo", "fields": [{"issue_field_id": 50325307, "issue_field_name": "msw-state", "value": "queued", "single_select_option": {"name": "queued"}}]}'
gql_before="$(graphql_calls)"
check "none sets the field" eq "$(ms set 48 none --base "$real_base" 2>&1)" "set #48 msw-state to none (was queued)"
check "and makes no GraphQL call, leaving the status alone" eq "$(( $(graphql_calls) - gql_before )) $(status_of 48)" "0 Todo"

seed 42 '{"fields": [{"issue_field_id": 999, "issue_field_name": "msw-state", "value": "queued", "single_select_option": {"name": "queued"}}]}'
posts_before="$(field_posts)"
check "a msw-state field with another id exits 1" ms_rc 1 set 42 implementing --base "$real_base"
check "and names the id and writes nothing" \
  eq "$(grep -c 'with id 999, not 50325307' "$sandbox/ms.out") $(( $(field_posts) - posts_before ))" "1 0"

seed 43 '{"fields_unavailable": true}'
check "unavailable issue fields exit 1" ms_rc 1 set 43 implementing --base "$real_base"
check "and point to the labels fallback" present "tracker.state.source: labels" "$sandbox/ms.out"
seed 44 '{"field_ignores_post": true, "status": "Todo"}'
mut_before="$(mutations)"
check "a POST that GitHub ignores exits 1" ms_rc 1 set 44 implementing --base "$real_base"
check "before any status write" eq "$(( $(mutations) - mut_before )) $(status_of 44)" "0 Todo"

seed 45 '{"status": "Todo"}'
posts_before="$(field_posts)" mut_before="$(mutations)"
out="$(ms set 45 planning --base "$real_base" --dry-run 2>&1)" || out="(failed) $out"
check "set --dry-run prints both lines" eq "$out" "set #45 msw-state to planning (was none)
set #45 status to In progress (was Todo)"
check "and writes nothing" \
  eq "$(( $(field_posts) - posts_before )) $(( $(mutations) - mut_before )) $(field_of 45)/$(status_of 45)" "0 0 /Todo"

state_cfg labels off > .mintswarm/config.yaml
seed 46 '{}'
ms set 46 queued --base "$real_base" >/dev/null 2>&1 || true
check "an uncommitted config edit changes nothing" eq "$(field_of 46) $(labels_of 46)" "queued "
git checkout -q -- .mintswarm/config.yaml

printf '%s\n' '[{"id": "S_todo", "name": "Todo", "category": "TODO"}]' > "$MSW_STUB_STATE/statuses.json"
seed 49 '{"status": "Todo"}'
check "a status name the repository lacks exits 1" ms_rc 1 set 49 "done" --base "$real_base"
check "and points to tracker.state.status: off" present "tracker.state.status: off" "$sandbox/ms.out"
rm -f "$MSW_STUB_STATE/statuses.json"
seed 50 '{"status": "Todo", "status_ignores_set": true}'
check "a setIssueStatus that GitHub ignores exits 1" ms_rc 1 set 50 implementing --base "$real_base"
check "and says so" present "did not set #50's status to In progress (it reports Todo)" "$sandbox/ms.out"
seed 51 '{"status": "Todo"}'
gql_before="$(graphql_calls)"
check "with status: off committed, set prints only the field line" \
  eq "$(ms set 51 implementing --base "$off_base" 2>&1)" "set #51 msw-state to implementing (was none)"
check "and makes no GraphQL call" eq "$(( $(graphql_calls) - gql_before )) $(status_of 51)" "0 Todo"
check "get with status: off prints only the value" eq "$(ms get 51 --base "$off_base" 2>&1)" "msw-state: implementing"

seed 61 '{"labels": ["bug"]}'
out="$(ms set 61 implementing --base "$labels_base" 2>&1)" || out="(failed) $out"
check "labels mode: set on an issue with no msw-state label" eq "$out" "set #61 msw-state to implementing (was none)"
check "creates the missing label once, colour ededed" \
  eq "$(repo_label_count msw-state:implementing) $(jq -r '.[0].color + " " + .[0].description' "$MSW_STUB_STATE/labels.json")" \
  "1 ededed mintswarm state"
check "and PUTs the issue's labels in one request, keeping the unrelated one" \
  gh_argv_seen '["api","--method","PUT","repos/{owner}/{repo}/issues/61/labels","--input","-"]'
check "the issue has bug and exactly one msw-state label" eq "$(labels_of 61)" "bug,msw-state:implementing"
out="$(ms set 61 'in review' --base "$labels_base" 2>&1)" || out="(failed) $out"
check "a new value swaps the label" eq "$out $(labels_of 61)" "set #61 msw-state to in review (was implementing) bug,msw-state:in review"
writes_before="$(label_writes)"
out="$(ms set 61 'in review' --base "$labels_base" 2>&1)" || out="(failed) $out"
check "the same value writes nothing" eq "$out $(( $(label_writes) - writes_before ))" "#61 msw-state is already in review 0"
check "get reads the value from the label" eq "$(ms get 61 --base "$labels_base" 2>&1)" "msw-state: in review"
seed 62 '{"labels": ["msw-state:queued", "msw-state:planning", "docs"]}'
out="$(ms set 62 "done" --base "$labels_base" 2>&1)" || out="(failed) $out"
check "an issue with two msw-state labels ends with one" eq "$out $(labels_of 62)" \
  "set #62 msw-state to done (was queued, planning) docs,msw-state:done"
seed 63 '{"labels": []}'
check "none sets msw-state:none" eq "$(ms set 63 none --base "$labels_base" 2>&1) $(labels_of 63)" \
  "set #63 msw-state to none (was none) msw-state:none"
seed 64 '{"labels": []}'
ms set 64 implementing --base "$labels_base" >/dev/null 2>&1 || true
check "an existing label is not created again" eq "$(repo_label_count msw-state:implementing) $(labels_of 64)" "1 msw-state:implementing"
seed 66 '{"labels": ["bug"]}'
check "labels mode needs no field or field_id" \
  eq "$(ms set 66 designing --base "$labels_only_base" 2>&1) $(labels_of 66)" \
  "set #66 msw-state to designing (was none) bug,msw-state:designing"
check "but field mode without field_id exits 2" exits_2 ms set 66 designing \
  --base "$(commit_config 'tracker: {state: {source: field, field: msw-state, label: "msw-state:{value}", status: off}}')"
jq -c '. + [{"name": "MSW-State:Awaiting Merge", "color": "ededed"}]' "$MSW_STUB_STATE/labels.json" > "$sandbox/labels.json"
mv "$sandbox/labels.json" "$MSW_STUB_STATE/labels.json"
seed 67 '{"labels": []}'
writes_before="$(label_writes)"
out="$(ms set 67 'awaiting merge' --base "$labels_base" 2>&1)" || out="(failed) $out"
check "a label that exists in another case is not created again" \
  eq "$out $(( $(label_writes) - writes_before )) $(repo_label_count 'msw-state:awaiting merge')" \
  "set #67 msw-state to awaiting merge (was none) 1 0"
seed 68 '{"labels": ["MSW-State:Awaiting Merge"]}'
writes_before="$(label_writes)"
out="$(ms set 68 'awaiting merge' --base "$labels_base" 2>&1)" || out="(failed) $out"
check "an issue label in another case is already the value" eq "$out $(( $(label_writes) - writes_before ))" \
  "#68 msw-state is already awaiting merge 0"
seed 65 '{"labels": [], "status": "Todo"}'
check "labels mode sets the status too" eq "$(ms set 65 analysing --base "$labels_status_base" 2>&1) $(status_of 65)" \
  "set #65 msw-state to analysing (was none)
set #65 status to In progress (was Todo) In progress"

cat > "$sandbox/mscheck.py" <<'PY'
# mscheck.py README SCRIPT: the README's msw-state tables agree with the script's VALUES and STATUS.
import importlib.util, re, sys

sys.dont_write_bytecode = True  # no __pycache__ next to the script
spec = importlib.util.spec_from_file_location("msw_state", sys.argv[2])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
text = open(sys.argv[1], encoding="utf-8").read()
section = text.split("\n## msw-state\n", 1)[1].split("\n## ", 1)[0]
tables, rows = {}, None
for line in section.split("\n"):
    if not line.startswith("|"):
        rows = None
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if rows is None:
        rows = tables[tuple(cells)] = []
    elif not set("".join(cells)) <= set("-: "):
        rows.append(cells)
errors = []
ticked = lambda cell: re.findall(r"`([^`]*)`", cell)
values = [ticked(r[0])[0] for r in tables.pop(("Value", "When it applies"))]
if values != list(mod.VALUES):
    errors.append("the Values table lists %s, not VALUES %s" % (values, list(mod.VALUES)))
status = {ticked(r[0])[0]: r[1] for r in tables.pop(("msw-state", "Issue status"))}
if status != dict(mod.STATUS, none="left alone"):
    errors.append("the Issue status table %s differs from STATUS" % status)
for head in (("Runner state", "msw-state"), ("Pipeline state", "msw-state")):
    for r in tables.pop(head):
        got = ticked(r[-1])
        if got and not all(v in mod.VALUES for v in got) or not got and r[-1] != "the value of the state it pauses":
            errors.append("%s row %s maps to no value in VALUES" % (head[0], r))
if tables:
    errors.append("unexpected tables: %s" % list(tables))
print("\n".join(errors))
sys.exit(1 if errors else 0)
PY
check "the README's msw-state tables match the script's VALUES and STATUS" python3 -I "$sandbox/mscheck.py" "$here/README.md" "$MS"
sed 's/| `escalated` | In review |/| `escalated` | Done |/' "$here/README.md" > "$sandbox/README-bad.md"
refuses "and the cross-check catches a status that differs" python3 -I "$sandbox/mscheck.py" "$sandbox/README-bad.md" "$MS"
sed 's/| `ready`, `merging`, `releasing` | `awaiting merge` |/| `ready`, `merging`, `releasing` | `waiting` |/' "$here/README.md" > "$sandbox/README-bad.md"
refuses "and a runner state mapped to an unknown value" python3 -I "$sandbox/mscheck.py" "$sandbox/README-bad.md" "$MS"

# --- summary -----------------------------------------------------------------

check "every curl call starts with -q, so ~/.curlrc is ignored" \
  eq "$(jq -r 'select(.cmd == "curl") | .argv[0]' "$argv_log" | sort -u)" "-q"
check "TMPDIR is empty at the end" tmp_empty
printf '\n%d passed, %d failed\n' "$passed" "$failed"
if [ "$failed" -eq 0 ]; then exit 0; fi
exit 1
