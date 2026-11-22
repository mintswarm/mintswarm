#!/usr/bin/env bash
# Offline self-test of the mintswarm-dev tooling in this directory.
#
#   selftest.sh
#
# Runs in a fresh mktemp -d sandbox with HOME, TMPDIR and MSW_APP_CONFIG inside it,
# and PATH starting with stubs for security, curl and gh that record their argv and
# stdin. It aborts before any check unless those three commands resolve to the
# stubs, so the real Keychain and GitHub are never reached. git runs without the
# system config, against repositories and a bare "remote" inside the sandbox.
#
# Checks: shellcheck and py_compile; manifest prepare and convert; the JWT and the
# installation token; key rotation; the bot identity include; push.sh pushes and
# refusals; the inline credential helper; bot-gh.sh; reviewer resolution. Secrets
# must appear in no recorded argv, nothing may be left in TMPDIR, HOME gains only
# dev-app.json, and .git/config stays byte-identical across pushes.
#
# Prints one line per check and exits 0 only if every check passed.
# Needs shellcheck, jq, yq, openssl, python3 and git.
#
# shellcheck disable=SC2016,SC2329
# SC2016: sh -c '...' scripts take their values as positional parameters.
# SC2329: check functions are invoked through check and refuses.
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
      GIT_ASKPASS SSH_ASKPASS
export HOME="$sandbox/home"
export TMPDIR="$sandbox/tmp"
export MSW_APP_CONFIG="$HOME/.config/mintswarm/dev-app.json"
export MSW_STUB_STATE="$sandbox/stub-state"
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
stubs="$sandbox/stubs"
keys="$sandbox/keys"
mkdir -p "$HOME" "$TMPDIR" "$MSW_STUB_STATE" "$stubs" "$keys"

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

if argv and argv[0] == "find-generic-password":
    key = (opt(argv, "-s") or "") + "/" + (opt(argv, "-a") or "")
    if key not in kc:
        print("security: The specified item could not be found in the keychain.", file=sys.stderr)
        sys.exit(44)
    print(kc[key] if "-w" in argv else 'keychain: "stub"')
    sys.exit(0)

sys.exit(1)
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
chmod 755 "$stubs/security" "$stubs/curl" "$stubs/gh"

export PATH="$stubs:$PATH"
for c in security curl gh; do
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

check "shellcheck on every script" sh -c 'cd "$1" && shellcheck -x ./*.sh' sh "$here"
check "request-review.py compiles" \
  python3 -c 'import py_compile, sys; py_compile.compile(sys.argv[1], cfile=sys.argv[2], doraise=True)' \
  "$R" "$sandbox/request-review.pyc"
no_secret_redirections() {
  local f
  for f in "$here"/*.sh; do
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
check "dev-app.json holds the five identifiers" \
  eq "$(jq -c 'keys' "$MSW_APP_CONFIG")" '["app_id","client_id","key_fingerprint","keychain_service","slug"]'
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

check "TMPDIR is empty at the end" tmp_empty
printf '\n%d passed, %d failed\n' "$passed" "$failed"
if [ "$failed" -eq 0 ]; then exit 0; fi
exit 1
