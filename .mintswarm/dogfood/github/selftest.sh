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
# Checks: shellcheck and py_compile; manifest prepare and convert; the JWT and the
# installation token; key rotation; every key store backend (selection, round
# trips, the file backend's modes and refusals, rotate and jwt through each); the
# bot identity include; push.sh pushes and refusals; the inline credential
# helper; bot-gh.sh; reviewer resolution. Secrets must appear in no recorded
# argv, nothing may be left in TMPDIR, HOME gains only dev-app.json, and
# .git/config stays byte-identical across pushes.
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
trap 'rm -rf "$sandbox"' EXIT
trap 'printf "selftest: setup failed at line %s\n" "$LINENO" >&2' ERR

unset MSW_TOKEN MSW_PUSH_URL GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN \
      GH_REPO GH_HOST GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR \
      GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_AUTHOR_DATE GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL \
      GIT_COMMITTER_DATE GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GIT_CONFIG_GLOBAL GIT_CONFIG \
      GIT_ASKPASS SSH_ASKPASS GIT_SSL_NO_VERIFY GIT_SSL_CAINFO GIT_SSL_CAPATH \
      MSW_KEY_STORE DBUS_SESSION_BUS_ADDRESS PASSWORD_STORE_DIR GNUPGHOME CREDENTIALS_DIRECTORY \
      MSW_STUB_UNAME MSW_STUB_SYSTEMD_VERSION
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
    for line in data.splitlines():
        words = shlex.split(line)
        if not words:
            continue
        if words[0] != "add-generic-password":
            sys.exit(1)
        s, a, w = opt(words, "-s"), opt(words, "-a"), opt(words, "-w")
        if not (s and a and w):
            sys.exit(1)
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

cat > "$stubs/curl" <<'EOF'
#!/usr/bin/env python3
# Stub of curl for the GitHub endpoints mint-token.sh and store-credentials.sh call.
import json, os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin

argv = sys.argv[1:]
record("curl", argv)
headers = ""
if any(argv[i] == "-H" and argv[i + 1] == "@-" for i in range(len(argv) - 1)):
    headers = sys.stdin.read()
    record_stdin("curl", headers)
if os.environ.get("MSW_STUB_CURL_FAIL"):
    sys.exit(22)
url = [a for a in argv if a.startswith("https://")][-1]
authed = re.search(r"^Authorization: Bearer [A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$", headers, re.M)

if url == "https://api.github.com/repos/mintswarm/mintswarm/installation" and authed:
    print(json.dumps({"id": 4242}))
elif url == "https://api.github.com/app/installations/4242/access_tokens" and authed:
    print(json.dumps({"token": os.environ["MSW_STUB_MINTED"], "expires_at": "2026-11-22T13:00:00Z"}))
elif url == "https://api.github.com/app-manifests/%s/conversions" % os.environ["MSW_STUB_CODE"]:
    pem = open(os.path.join(STATE, "conv-key.pem")).read()
    print(json.dumps({"id": 5874301, "slug": "mintswarm-dev", "client_id": os.environ["MSW_STUB_CLIENT_ID"],
                      "client_secret": "selftest-client-secret-value", "webhook_secret": "selftest-webhook-secret",
                      "pem": pem}))
else:
    print("curl: (22) The requested URL returned error: 404", file=sys.stderr)
    sys.exit(22)
EOF

cat > "$stubs/gh" <<'EOF'
#!/usr/bin/env python3
# Stub of gh: records argv, the environment it was given and stdin for --input -.
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from stublib import STATE, record, record_stdin

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
if argv[:1] == ["api"] and any(a.endswith("/requested_reviewers") for a in argv):
    reviewers = json.loads(data)["reviewers"]
    print(json.dumps({"number": 7, "requested_reviewers": [{"login": r} for r in reviewers]}))
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
fingerprint_of() { openssl pkey -in "$1" -pubout -outform DER | openssl sha256 -binary | openssl base64; }
ident() { git log -1 --format='%an|%ae|%cn|%ce' "$@"; }
S="$here/store-credentials.sh"
M="$here/mint-token.sh"
P="$here/push.sh"
G="$here/bot-gh.sh"
R="$here/request-review.py"

# --- static checks -----------------------------------------------------------

check "shellcheck on every script" sh -c 'cd "$1" && shellcheck -x ./*.sh linux/*.sh' sh "$here"
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

# --- App creation: prepare and convert ---------------------------------------

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$keys/key1.pem" 2>/dev/null
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$keys/key2.pem" 2>/dev/null
openssl pkey -in "$keys/key1.pem" -pubout -out "$keys/pub1.pem"
openssl pkey -in "$keys/key2.pem" -pubout -out "$keys/pub2.pem"
cp "$keys/key1.pem" "$MSW_STUB_STATE/conv-key.pem"
key1_b64="$(openssl base64 -A -in "$keys/key1.pem")"
key2_b64="$(openssl base64 -A -in "$keys/key2.pem")"
key1_line="$(sed -n 2p "$keys/key1.pem")"
key2_line="$(sed -n 2p "$keys/key2.pem")"

"$S" prepare > "$sandbox/prepare.out"
state="$(cat "$HOME/.config/mintswarm/manifest-state")"
check "prepare writes the page with the state filled in" \
  grep -qF "settings/apps/new?state=$state\"" "$HOME/.config/mintswarm/create-app.html"
check "the tracked page keeps its placeholder" grep -qF 'state=REPLACE_WITH_RANDOM' "$here/create-app.html"

home_files > "$sandbox/home-before"
refuses "convert refuses a mismatched state" "$S" convert "$CODE" not-the-state
refuses "convert refuses a malformed code" "$S" convert 'x/../y' "$state"
check "convert succeeds" sh -c '"$1" convert "$2" "$3" > "$4"' sh "$S" "$CODE" "$state" "$sandbox/convert.out"
home_files > "$sandbox/home-after"
check "HOME gains only dev-app.json" \
  eq "$(LC_ALL=C comm -13 "$sandbox/home-before" "$sandbox/home-after")" "./.config/mintswarm/dev-app.json"
check "convert removes the manifest state" \
  eq "$(LC_ALL=C comm -23 "$sandbox/home-before" "$sandbox/home-after")" "./.config/mintswarm/manifest-state"
check "dev-app.json holds the five identifiers and the key store" \
  eq "$(jq -c 'keys' "$MSW_APP_CONFIG")" '["app_id","client_id","key_fingerprint","key_store","keychain_service","slug"]'
check "dev-app.json records the keychain key store" eq "$(jq -r .key_store "$MSW_APP_CONFIG")" keychain
check "dev-app.json records the key's fingerprint" \
  eq "$(jq -r .key_fingerprint "$MSW_APP_CONFIG")" "SHA256:$(fingerprint_of "$keys/key1.pem")"
check "dev-app.json holds no secret" absent selftest-client-secret-value "$MSW_APP_CONFIG"
check "no secret anywhere under HOME" \
  sh -c '! grep -rqE "selftest-client-secret|selftest-webhook|PRIVATE KEY" "$1"' sh "$HOME"
check "key base64 in no argv" absent "$key1_b64" "$argv_log"
check "key PEM in no argv" absent "$key1_line" "$argv_log"
check "no PRIVATE KEY in any argv" absent "PRIVATE KEY" "$argv_log"
check "key reaches security -i on stdin" present "-w $key1_b64" "$MSW_STUB_STATE/stdin-security.log"
check "the Keychain label is passed quoted" \
  present '-l "mintswarm-dev GitHub App private key"' "$MSW_STUB_STATE/stdin-security.log"
check "TMPDIR is empty after convert" tmp_empty

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
curl_calls() { jq -r 'select(.cmd == "curl") | .cmd' "$argv_log" | wc -l | tr -d ' '; }
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

# --- key store ---------------------------------------------------------------

K="$here/key-store.sh"
ks_root="$sandbox/ks"   # HOME and stores for these checks, outside the main HOME
mkdir -p "$ks_root/sel" "$ks_root/home" "$ks_root/pass" "$ks_root/xdg" "$ks_root/creds"
printf 'selftest-gpg-id\n' > "$ks_root/pass/.gpg-id"

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
with_value() { printf 'abc\n' | "$@"; }   # with_value COMMAND...: COMMAND with a dummy value on stdin
linux_sel() { env HOME="$ks_root/sel" MSW_STUB_UNAME=Linux "$@"; }
no_key_dir() { test ! -e "$ks_root/sel/.config/mintswarm/keys"; }

for n in '' .. .x -x a/b 'a b' 'a$b'; do
  check "key-store.sh refuses the service name '$n' as bad usage" exits_2 env MSW_KEY_STORE=keychain "$K" get "$n" acct
  check "key-store.sh refuses the account name '$n' as bad usage" exits_2 env MSW_KEY_STORE=keychain "$K" get svc "$n"
done
check "key-store.sh refuses a label with a quote" exits_2 env MSW_KEY_STORE=keychain "$K" put svc acct 'a"b'
check "key-store.sh without arguments is bad usage" exits_2 "$K"
check "key-store.sh refuses an extra argument" exits_2 "$K" get svc acct extra
check "refused names reached no key store" eq "$(jq -r 'select(.cmd == "security") | .argv[0]' "$argv_log" | grep -c -e delete -e svc)" 0

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
env HOME="$ks_root/sel" "$S" prepare > /dev/null
conv_state="$(cat "$ks_root/sel/.config/mintswarm/manifest-state")"
calls="$(curl_calls)"
check "convert with no key store fails before using the code" \
  fails_saying "set MSW_KEY_STORE to one of" linux_sel "$S" convert "$CODE" "$conv_state"
check "and calls no GitHub endpoint" eq "$(curl_calls)" "$calls"
check "the file backend was never used without MSW_KEY_STORE=file" no_key_dir
check "the failed selections ran no secret-tool or pass" \
  eq "$(jq -r 'select(.cmd == "secret-tool" or .cmd == "pass") | .cmd' "$argv_log" | wc -l | tr -d ' ')" 0

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

# --- summary -----------------------------------------------------------------

check "every curl call starts with -q, so ~/.curlrc is ignored" \
  eq "$(jq -r 'select(.cmd == "curl") | .argv[0]' "$argv_log" | sort -u)" "-q"
check "TMPDIR is empty at the end" tmp_empty
printf '\n%d passed, %d failed\n' "$passed" "$failed"
if [ "$failed" -eq 0 ]; then exit 0; fi
exit 1
