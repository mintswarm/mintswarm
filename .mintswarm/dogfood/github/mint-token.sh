#!/usr/bin/env bash
# Mint a GitHub App credential for mintswarm-dev without the key touching disk.
#
#   mint-token.sh jwt                  print an App JWT (10 minutes; for /app endpoints)
#   mint-token.sh token [PERMS_JSON]   print an installation token for $MSW_REPO (1 hour)
#
# Identifiers come from $MSW_APP_CONFIG (default ~/.config/mintswarm/dev-app.json),
# the private key from the key store recorded there as key_store (keychain when
# the field is missing on macOS; see key-store.sh). $MSW_KEY_STORE is ignored
# here, so the key is never read from a different store than it was written to.
# PERMS_JSON narrows the token, for example '{"contents":"read"}'.
#
# The key reaches openssl only through a pipe from key-store.sh. The JWT reaches curl on stdin
# (-H @-), never in argv. The only temporary file holds the non-secret JWT
# signing input (header and payload) and is removed on exit. curl runs with -q
# first, so ~/.curlrc cannot add tracing or verbose output.
#
# The mode is required: a bare mint-token.sh prints usage, not a token.
#
# Do not run "token" bare in a session whose output is recorded: the token is
# printed. push.sh and bot-gh.sh mint their own.
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() { printf '%s\n' "usage: mint-token.sh jwt | token [PERMS_JSON]" >&2; exit 2; }

[ -f "$MSW_APP_CONFIG" ] || msw_die "no App config at $MSW_APP_CONFIG (see README.md, Setup)"
client_id=""
service=""
client_id="$(jq -r '.client_id // empty' "$MSW_APP_CONFIG")" || msw_die "cannot read $MSW_APP_CONFIG"
service="$(jq -r '.keychain_service // empty' "$MSW_APP_CONFIG")" || msw_die "cannot read $MSW_APP_CONFIG"
[ -n "$client_id" ] || msw_die "client_id missing from $MSW_APP_CONFIG"
[ -n "$service" ] || msw_die "keychain_service missing from $MSW_APP_CONFIG"
store=""
store="$(msw_recorded_key_store)" || exit 1

b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

sign_input=""
cleanup() { if [ -n "$sign_input" ]; then rm -f "$sign_input"; fi; }

jwt() {
  local now header payload signature
  now="$(date +%s)"
  header="$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)"
  payload="$(printf '{"iat":%d,"exp":%d,"iss":"%s"}' $((now - 60)) $((now + 540)) "$client_id" | b64url)"
  sign_input="$(mktemp)"
  trap cleanup EXIT
  printf '%s.%s' "$header" "$payload" > "$sign_input"
  signature="$(MSW_KEY_STORE="$store" "$MSW_HERE/key-store.sh" get "$service" "$client_id" \
    | openssl dgst -sha256 -binary -sign /dev/stdin "$sign_input" \
    | b64url)" || signature=""
  cleanup
  sign_input=""
  [ -n "$signature" ] || msw_die "signing the JWT failed (is the key in the $store key store?)"
  printf '%s.%s.%s' "$header" "$payload" "$signature"
}

api() {  # api METHOD PATH [DATA], authenticated with $JWT
  local method="$1" path="$2" data="${3:-}"
  if [ -n "$data" ]; then
    printf 'Authorization: Bearer %s\n' "$JWT" | curl -q -fsS -X "$method" \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      -H @- \
      -d "$data" \
      "https://api.github.com$path"
  else
    printf 'Authorization: Bearer %s\n' "$JWT" | curl -q -fsS -X "$method" \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      -H @- \
      "https://api.github.com$path"
  fi
}

mode="${1:-}"
case "$mode" in
  jwt)
    [ $# -le 1 ] || usage
    jwt; echo ;;
  token)
    [ $# -le 2 ] || usage
    perms="${2:-}"
    if [ -n "$perms" ]; then
      printf '%s' "$perms" | jq -e 'type == "object"' >/dev/null 2>&1 \
        || msw_die "PERMS_JSON must be a JSON object, for example '{\"contents\":\"read\"}'"
    fi
    JWT=""
    JWT="$(jwt)"
    response=""
    response="$(api GET "/repos/$MSW_REPO_OWNER/$MSW_REPO_NAME/installation")" \
      || msw_die "looking up the installation on $MSW_REPO_OWNER/$MSW_REPO_NAME failed"
    installation=""
    installation="$(printf '%s' "$response" | jq -r '.id // empty')"
    case "$installation" in
      ''|*[!0-9]*) msw_die "no installation id for $MSW_REPO_OWNER/$MSW_REPO_NAME" ;;
    esac
    body="$(jq -nc --arg repo "$MSW_REPO_NAME" --argjson perms "${perms:-null}" \
      '{repositories: [$repo]} + (if $perms then {permissions: $perms} else {} end)')"
    response="$(api POST "/app/installations/$installation/access_tokens" "$body")" \
      || msw_die "creating the installation token failed"
    token=""
    token="$(printf '%s' "$response" | jq -r '.token // empty')"
    [ -n "$token" ] || msw_die "the response held no token"
    printf '%s\n' "$token" ;;
  *)
    usage ;;
esac
