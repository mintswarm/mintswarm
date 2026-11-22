#!/usr/bin/env bash
# Create the mintswarm-dev App from its manifest, or rotate its private key.
#
#   store-credentials.sh prepare
#       Generate a random manifest state, keep it in ~/.config/mintswarm/manifest-state
#       and write ~/.config/mintswarm/create-app.html with it filled in. Open that
#       file in a browser signed in as an organisation owner.
#   store-credentials.sh convert CODE STATE
#       After GitHub redirects to http://localhost:8765/?code=CODE&state=STATE:
#       check STATE, exchange CODE for the App's credentials, store the private
#       key in the key store (key-store.sh: $MSW_KEY_STORE, or the first
#       available) and write the public identifiers and the key store's name to
#       $MSW_APP_CONFIG (default ~/.config/mintswarm/dev-app.json). The client
#       secret and webhook secret are discarded.
#   store-credentials.sh rotate PEM_PATH [--keep]
#       Store a private key downloaded from the App's settings under the same
#       service and account, in $MSW_KEY_STORE when set and otherwise in the key
#       store recorded in dev-app.json; update key_fingerprint and key_store in
#       dev-app.json and delete the downloaded file unless --keep. Moving to
#       another key store leaves the old item where it is.
#
# The key passes only through shell variables and pipes: it reaches key-store.sh
# on stdin, never in argv, a temporary file or a here-string, and is read back
# to compare fingerprints. curl runs with -q first, so ~/.curlrc cannot trace the
# conversion response. dev-app.json holds no secret: app_id, slug, client_id,
# keychain_service (the key's service name in every key store), key_store,
# key_fingerprint.
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

KEY_LABEL='mintswarm-dev GitHub App private key'
STATE_FILE="$MSW_CONFIG_DIR/manifest-state"
umask 077

usage() {
  printf '%s\n' "usage: store-credentials.sh prepare | convert CODE STATE | rotate PEM_PATH [--keep]" >&2
  exit 2
}

check_word() {  # check_word WHAT VALUE: VALUE must be a plain word
  case "$2" in
    ''|*[!A-Za-z0-9._-]*) msw_die "unexpected $1: '$2'" ;;
  esac
}

fingerprint() {  # stdin: a PEM private key; stdout: base64 SHA-256 of its DER public key
  openssl pkey -passin pass: -pubout -outform DER | openssl sha256 -binary | openssl base64
}

KEY_FINGERPRINT=""
import_key() {  # import_key SERVICE CLIENT_ID PEM STORE; sets KEY_FINGERPRINT
  local service="$1" client_id="$2" pem="$3" store="$4" stored
  check_word "keychain service" "$service"
  check_word "client id" "$client_id"
  printf '%s\n' "$pem" | openssl pkey -passin pass: -noout 2>/dev/null \
    || msw_die "not an unencrypted private key"
  KEY_FINGERPRINT="$(printf '%s\n' "$pem" | fingerprint)"
  printf '%s\n' "$pem" | MSW_KEY_STORE="$store" "$MSW_HERE/key-store.sh" put "$service" "$client_id" "$KEY_LABEL" \
    || msw_die "storing the key in the $store key store failed"
  stored="$(MSW_KEY_STORE="$store" "$MSW_HERE/key-store.sh" get "$service" "$client_id" | fingerprint)" \
    || msw_die "reading the key back from the $store key store failed"
  [ "$stored" = "$KEY_FINGERPRINT" ] \
    || msw_die "the key in the $store key store does not match the one stored"
}

prepare() {
  local state
  state="$(openssl rand -hex 16)"
  mkdir -p "$MSW_CONFIG_DIR"
  printf '%s\n' "$state" > "$STATE_FILE"
  sed "s/REPLACE_WITH_RANDOM/$state/" "$MSW_HERE/create-app.html" > "$MSW_CONFIG_DIR/create-app.html"
  printf '%s\n' "open in a browser signed in as an owner of the $MSW_REPO_OWNER organisation:"
  printf '%s\n' "$MSW_CONFIG_DIR/create-app.html"
  printf '%s\n' "then run: store-credentials.sh convert CODE STATE, with both from the redirect URL"
}

convert() {
  local code="$1" state="$2" expected conv id slug client_id pem store
  check_word "code" "$code"
  [ -f "$STATE_FILE" ] || msw_die "no $STATE_FILE: run 'store-credentials.sh prepare' first"
  expected="$(cat "$STATE_FILE")"
  [ "$state" = "$expected" ] || msw_die "state mismatch: not the request we started"
  # A code works once: fail before using it when there is nowhere to keep the key.
  "$MSW_HERE/key-store.sh" backend >/dev/null || exit 1

  conv="$(curl -q -fsS -X POST -H "Accept: application/vnd.github+json" \
    "https://api.github.com/app-manifests/${code}/conversions")" \
    || msw_die "the manifest conversion failed (codes expire after an hour and work once)"
  id="$(printf '%s' "$conv" | jq -r '.id // empty')"
  slug="$(printf '%s' "$conv" | jq -r '.slug // empty')"
  client_id="$(printf '%s' "$conv" | jq -r '.client_id // empty')"
  pem="$(printf '%s' "$conv" | jq -r '.pem // empty')"
  conv=""
  case "$id" in ''|*[!0-9]*) msw_die "the conversion response held no App id" ;; esac
  [ -n "$slug" ] || msw_die "the conversion response held no slug"
  [ -n "$pem" ] || msw_die "the conversion response held no private key"
  check_word "client id" "$client_id"
  store="$("$MSW_HERE/key-store.sh" backend "$MSW_KEYCHAIN_SERVICE" "$client_id")" || exit 1

  import_key "$MSW_KEYCHAIN_SERVICE" "$client_id" "$pem" "$store"
  pem=""

  mkdir -p "$(dirname "$MSW_APP_CONFIG")"
  jq -n --arg id "$id" --arg slug "$slug" --arg client_id "$client_id" \
        --arg keychain_service "$MSW_KEYCHAIN_SERVICE" --arg key_store "$store" \
        --arg fingerprint "SHA256:$KEY_FINGERPRINT" \
        '{app_id: ($id|tonumber), slug: $slug, client_id: $client_id, keychain_service: $keychain_service, key_store: $key_store, key_fingerprint: $fingerprint}' \
    > "$MSW_APP_CONFIG"
  rm -f "$STATE_FILE"

  printf '%s\n' "stored: app $slug (id $id, client $client_id), key in the $store key store"
  printf '%s\n' "key fingerprint SHA256:$KEY_FINGERPRINT (compare with the App's settings page)"
  printf '%s\n' "identifiers: $MSW_APP_CONFIG"
}

rotate() {
  local path="$1" keep="$2" pem service client_id updated store old
  [ -f "$path" ] || msw_die "no such file: $path"
  [ -f "$MSW_APP_CONFIG" ] || msw_die "no App config at $MSW_APP_CONFIG: rotate needs an existing App"
  service="$(jq -r '.keychain_service // empty' "$MSW_APP_CONFIG")"
  client_id="$(jq -r '.client_id // empty' "$MSW_APP_CONFIG")"
  old="$(msw_recorded_key_store 2>/dev/null)" || old=""
  store="${MSW_KEY_STORE:-}"
  if [ -z "$store" ]; then
    store="$(msw_recorded_key_store)" || exit 1
  fi
  pem="$(cat "$path")"

  import_key "$service" "$client_id" "$pem" "$store"
  pem=""

  updated="$(jq --arg fp "SHA256:$KEY_FINGERPRINT" --arg ks "$store" \
    '.key_fingerprint = $fp | .key_store = $ks' "$MSW_APP_CONFIG")"
  printf '%s\n' "$updated" > "$MSW_APP_CONFIG"
  printf '%s\n' "key stored in the $store key store"
  if [ -n "$old" ] && [ "$old" != "$store" ]; then
    printf '%s\n' "the previous key is still in the $old key store; once the new one works, delete it with:"
    printf '%s\n' "  MSW_KEY_STORE=$old $MSW_HERE/key-store.sh delete $service $client_id"
  fi
  if [ "$keep" = 1 ]; then
    printf '%s\n' "kept $path: delete it once the new key works"
  else
    rm -f "$path"
    printf '%s\n' "deleted $path"
  fi
  printf '%s\n' "key fingerprint SHA256:$KEY_FINGERPRINT (compare with the App's settings page)"
  printf '%s\n' "next: mint a token (for example bot-gh.sh api repos/{owner}/{repo} --jq .full_name),"
  printf '%s\n' "then delete the old key in the App's settings on GitHub"
}

case "${1:-}" in
  prepare)
    [ $# -eq 1 ] || usage
    prepare ;;
  convert)
    [ $# -eq 3 ] || usage
    convert "$2" "$3" ;;
  rotate)
    case "$#:${3:-}" in
      2:) rotate "$2" 0 ;;
      3:--keep) rotate "$2" 1 ;;
      *) usage ;;
    esac ;;
  *)
    usage ;;
esac
