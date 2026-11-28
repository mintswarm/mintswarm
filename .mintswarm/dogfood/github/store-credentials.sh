#!/usr/bin/env bash
# Create a GitHub App from its manifest, record its bot user id, or rotate an App's
# private key.
#
#   store-credentials.sh create ROLE MANIFEST
#       Run the whole manifest flow for the App of ROLE (bee or keeper; see
#       README.md, Roles) that MANIFEST (a JSON file, see manifests/) describes.
#       ROLE picks the profile in common.sh (msw_app_profile): the role's own
#       config file. The key's service and label are derived from the
#       manifest's name, which GitHub must return as the slug:
#       <slug>-private-key and "<slug> GitHub App private key".
#       Before anything is shown it checks the manifest (a plain name, public
#       false, redirect_url http://127.0.0.1:PORT/ with PORT from 1024 to
#       65535), that the role's config does not exist yet and is not the other
#       role's, that MSW_MANIFEST_TIMEOUT (seconds, default 600) is valid and
#       that a key store is available. It then writes
#       ~/.config/mintswarm/create-NAME.html (removed on exit), which posts the
#       manifest to GitHub with a random state, and starts manifest-listener.py
#       on 127.0.0.1:PORT. Once the listener says it is listening, open the page
#       it names in a browser signed in as an owner of the organisation. The
#       listener takes the first redirect whose state matches and hands the code over on a pipe; curl
#       exchanges it for the App's credentials, reading the URL from its stdin
#       (-K -). The App must come back with the manifest's name as slug and
#       the organisation as owner, or nothing is stored. The App's bot user
#       id is then looked up as bot-id does, and nothing is stored when that
#       fails. The private key goes to the App's own key-store item
#       (key-store.sh put: $MSW_KEY_STORE, or the first available), without
#       per-use confirmation, and the public
#       identifiers, bot_user_id included, to the role's config, with
#       key_confirm none. The client secret and webhook secret are discarded.
#   store-credentials.sh bot-id
#       Record the bot user id of the App whose config $MSW_APP_CONFIG names
#       (default ~/.config/mintswarm/bee-app.json) as bot_user_id in that
#       config, changing nothing else. The id is read from the public
#       https://api.github.com/users/<slug>%5Bbot%5D, with no credential, and
#       kept only when the answer's type is Bot, its login <slug>[bot] and its
#       id all digits; otherwise nothing is stored. The bot's noreply e-mail
#       is derived from it (common.sh, msw_bot_email).
#   store-credentials.sh rotate PEM_PATH [--keep]
#       Store a private key downloaded from the App's settings under the same
#       service and account, for the App whose config $MSW_APP_CONFIG names
#       (default ~/.config/mintswarm/bee-app.json), in $MSW_KEY_STORE when set
#       and otherwise in the key store recorded there; update key_fingerprint,
#       key_store and key_confirm (none) and delete the downloaded file unless
#       --keep. The service is the config's keychain_service, and the label
#       is derived from the config's slug. The key is stored without per-use
#       confirmation; a config whose key_confirm is neither none nor absent is
#       refused, storing nothing.
#       Moving to another key store leaves the old item where it is.
#
# The code passes only through a pipe, a shell variable and curl's stdin; it is
# never in argv, a file or the output. The key passes only through shell
# variables and pipes: it reaches key-store.sh on stdin, never in argv, a
# temporary file or a here-string, and is read back to compare fingerprints.
# curl runs with -q first, so ~/.curlrc cannot trace the conversion response.
# An App's config holds no secret: app_id, slug, client_id, keychain_service
# (the key's service name in every key store), key_store, key_fingerprint,
# bot_user_id (the number in the bot's noreply e-mail), key_confirm (always
# none: no App's key needs per-use confirmation; a config without it means
# none).
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

umask 077

usage() {
  printf '%s\n' "usage: store-credentials.sh create ROLE MANIFEST | bot-id | rotate PEM_PATH [--keep]" >&2
  exit 2
}

check_word() {  # check_word WHAT VALUE: VALUE must be a plain word
  case "$2" in
    ''|*[!A-Za-z0-9._-]*) msw_die "unexpected $1: '$2'" ;;
  esac
}

key_label() { printf '%s GitHub App private key\n' "$1"; }  # key_label SLUG

BOT_USER_ID=""
lookup_bot_id() {  # lookup_bot_id SLUG: sets BOT_USER_ID from GitHub's public answer for SLUG[bot]
  local slug="$1" answer id
  BOT_USER_ID=""
  check_word "slug" "$slug"
  answer="$(curl -q -fsS -H "Accept: application/vnd.github+json" "https://api.github.com/users/$slug%5Bbot%5D")" \
    || msw_die "cannot read the bot user ${slug}[bot] from GitHub: nothing was stored"
  printf '%s' "$answer" | jq -e --arg login "${slug}[bot]" \
      'type == "object" and .login == $login and .type == "Bot"' >/dev/null 2>&1 \
    || msw_die "GitHub's answer for ${slug}[bot] is not that bot user: nothing was stored"
  id="$(printf '%s' "$answer" | jq -r '.id | numbers | tostring' 2>/dev/null)" || id=""
  case "$id" in
    ''|*[!0-9]*) msw_die "GitHub's answer for ${slug}[bot] holds no numeric id: nothing was stored" ;;
  esac
  BOT_USER_ID="$id"
}

fingerprint() {  # stdin: a PEM private key; stdout: base64 SHA-256 of its DER public key
  openssl pkey -passin pass: -pubout -outform DER | openssl sha256 -binary | openssl base64
}

canon() {  # canon PATH: PATH with its existing directories resolved, for comparing config paths
  local d rest p
  d="$(dirname "$1")"
  rest="$(basename "$1")"
  while [ ! -d "$d" ]; do
    rest="$(basename "$d")/$rest"
    d="$(dirname "$d")"
  done
  p="$(cd "$d" && pwd -P)"
  printf '%s/%s\n' "${p%/}" "$rest"
}

KEY_FINGERPRINT=""
KEY_PUT=""  # after a successful put: the command that deletes the item again
import_key() {  # import_key SERVICE CLIENT_ID PEM STORE LABEL; sets KEY_FINGERPRINT
  local service="$1" client_id="$2" pem="$3" store="$4" label="$5" stored
  check_word "keychain service" "$service"
  check_word "client id" "$client_id"
  printf '%s\n' "$pem" | openssl pkey -passin pass: -noout 2>/dev/null \
    || msw_die "not an unencrypted private key"
  KEY_FINGERPRINT="$(printf '%s\n' "$pem" | fingerprint)"
  printf '%s\n' "$pem" | MSW_KEY_STORE="$store" "$MSW_HERE/key-store.sh" put "$service" "$client_id" "$label" \
    || msw_die "storing the key in the $store key store failed"
  KEY_PUT="MSW_KEY_STORE=$store $MSW_HERE/key-store.sh delete $service $client_id"
  stored="$(MSW_KEY_STORE="$store" "$MSW_HERE/key-store.sh" get "$service" "$client_id" | fingerprint)" \
    || msw_die "reading the key back from the $store key store failed"
  [ "$stored" = "$KEY_FINGERPRINT" ] \
    || msw_die "the key in the $store key store does not match the one stored"
}

# --- create ------------------------------------------------------------------

PAGE=""
PENDING=""  # set while an App exists on GitHub and create has not finished
on_exit() {
  if [ -n "$PAGE" ]; then rm -f "$PAGE"; fi
  [ -n "$PENDING" ] || return 0
  if [ -n "$KEY_PUT" ]; then
    printf '%s\n' "store-credentials.sh: $PENDING exists on GitHub and its key is in the key store, but create did not finish." >&2
    printf '%s\n' "store-credentials.sh: delete the App (organisation settings, GitHub Apps) and the key ($KEY_PUT), then run create again." >&2
  else
    printf '%s\n' "store-credentials.sh: $PENDING exists on GitHub, but its key was not stored." >&2
    printf '%s\n' "store-credentials.sh: delete it (organisation settings, GitHub Apps) and run create again." >&2
  fi
}

create() {
  local role="$1" manifest="$2"
  local name redirect port cfg other timeout state compact service label
  local code conv id slug owner client_id pem perms_ok events_ok store

  # Everything that can fail before GitHub is involved fails here.
  case "$role" in
    bee|keeper) msw_app_profile "$role" ;;
    *) msw_die "no profile for the role '$role' (known: bee, keeper; envoy is reserved)" ;;
  esac
  [ -f "$manifest" ] || msw_die "no such manifest: $manifest"
  jq -e 'type == "object"' "$manifest" >/dev/null 2>&1 || msw_die "$manifest is not a JSON object"
  name="$(jq -r '.name // empty | strings' "$manifest")"
  case "$name" in
    ''|*[!A-Za-z0-9-]*) msw_die "unexpected App name in $manifest: '$name' (a slug: letters, digits and -)" ;;
  esac
  service="$name-private-key"
  label="$(key_label "$name")"
  redirect="$(jq -r '.redirect_url // empty | strings' "$manifest")"
  case "$redirect" in
    http://127.0.0.1:*/) port="${redirect#http://127.0.0.1:}"; port="${port%/}" ;;
    *) msw_die "redirect_url in $manifest must be http://127.0.0.1:PORT/" ;;
  esac
  case "$port" in
    ''|0*|*[!0-9]*) msw_die "redirect_url in $manifest must be http://127.0.0.1:PORT/" ;;
  esac
  { [ "${#port}" -le 5 ] && [ "$port" -ge 1024 ] && [ "$port" -le 65535 ]; } \
    || msw_die "the redirect port in $manifest must be from 1024 to 65535"
  jq -e '.public == false' "$manifest" >/dev/null || msw_die "$manifest must say \"public\": false"

  cfg="$MSW_PROFILE_CONFIG"
  case "$role" in
    keeper)
      for other in "$MSW_APP_CONFIG" "$MSW_CONFIG_DIR/bee-app.json"; do
        [ "$(canon "$cfg")" != "$(canon "$other")" ] \
          || msw_die "the keeper's config $cfg is the bee's config: set MSW_KEEPER_CONFIG to a file of its own"
      done ;;
    *)
      [ "$(canon "$cfg")" != "$(canon "$MSW_KEEPER_CONFIG")" ] \
        || msw_die "the bee's config $cfg is the keeper's config: set MSW_APP_CONFIG to a file of its own" ;;
  esac
  if [ -e "$cfg" ] || [ -L "$cfg" ]; then
    msw_die "$cfg exists: the $role's App has been created already (move the file aside to create it again)"
  fi

  timeout="${MSW_MANIFEST_TIMEOUT:-600}"
  case "$timeout" in
    ''|0*|*[!0-9]*) msw_die "MSW_MANIFEST_TIMEOUT must be a whole number of seconds above 0" ;;
  esac

  # A code works once: fail before asking for one when there is nowhere to keep the key.
  "$MSW_HERE/key-store.sh" backend >/dev/null || exit 1

  # The page that posts the manifest, with a state held only here and in it.
  state="$(openssl rand -hex 16)"
  case "$state" in
    ????????????????????????????????) ;;
    *) msw_die "openssl rand gave no state" ;;
  esac
  case "$state" in *[!0-9a-f]*) msw_die "openssl rand gave no state" ;; esac
  compact="$(jq -c . "$manifest")"
  mkdir -p "$MSW_CONFIG_DIR"
  PAGE="$MSW_CONFIG_DIR/create-$name.html"
  jq -rn --arg action "https://github.com/organizations/$MSW_REPO_OWNER/settings/apps/new?state=$state" \
         --arg manifest "$compact" --arg name "$name" '
    "<!doctype html>",
    "<meta charset=\"utf-8\">",
    "<title>Create the \($name | @html) GitHub App</title>",
    "<form id=\"f\" method=\"post\" action=\"\($action | @html)\">",
    "<input type=\"hidden\" name=\"manifest\" value=\"\($manifest | @html)\">",
    "<p>Sending the manifest to GitHub. If nothing happens, press the button.</p>",
    "<button type=\"submit\">Create \($name | @html) on GitHub</button>",
    "</form>",
    "<script>document.getElementById(\"f\").submit();</script>"' > "$PAGE"
  chmod 600 "$PAGE"

  printf '%s\n' "creating $name: once the listener says it is listening, open the page it names in a browser"
  printf '%s\n' "signed in as an owner of the $MSW_REPO_OWNER organisation; it waits up to $timeout seconds for GitHub's redirect to $redirect"
  code="$(printf '%s\n' "$state" | python3 -I "$MSW_HERE/manifest-listener.py" "$port" "$timeout" "$PAGE")" \
    || msw_die "no code was received, so no App was converted (if GitHub created $name anyway, delete it and run create again)"
  state=""
  rm -f "$PAGE"
  PAGE=""
  # Checked again here, without showing it.
  case "$code" in
    ''|*[!A-Za-z0-9._-]*) code=""; msw_die "the listener returned a malformed code" ;;
  esac
  [ "${#code}" -le 256 ] || { code=""; msw_die "the listener returned a malformed code"; }

  PENDING="the App $name (its id is unknown until the conversion answers)"
  conv="$(printf 'url = "https://api.github.com/app-manifests/%s/conversions"\n' "$code" \
          | curl -q -fsS -X POST -H "Accept: application/vnd.github+json" -K -)" \
    || { code=""; msw_die "the manifest conversion failed (codes expire after an hour and work once)"; }
  code=""
  id="$(printf '%s' "$conv" | jq -r '.id // empty')"
  slug="$(printf '%s' "$conv" | jq -r '.slug // empty')"
  owner="$(printf '%s' "$conv" | jq -r '.owner.login // empty')"
  client_id="$(printf '%s' "$conv" | jq -r '.client_id // empty')"
  pem="$(printf '%s' "$conv" | jq -r '.pem // empty')"
  perms_ok="$(printf '%s' "$conv" | jq --slurpfile m "$manifest" '(.permissions // null) == $m[0].default_permissions')"
  events_ok="$(printf '%s' "$conv" | jq --slurpfile m "$manifest" '(.events // null) == $m[0].default_events')"
  conv=""
  case "$id" in ''|*[!0-9]*) msw_die "the conversion response held no App id" ;; esac
  check_word "slug" "$slug"
  PENDING="the App $slug (id $id)"
  if [ "$slug" != "$name" ] || [ "$owner" != "$MSW_REPO_OWNER" ]; then
    pem=""
    msw_die "GitHub created the App $slug (id $id) owned by '$owner', not $name owned by $MSW_REPO_OWNER: nothing was stored"
  fi
  [ -n "$pem" ] || msw_die "the conversion response held no private key"
  check_word "client id" "$client_id"
  [ "$perms_ok" = true ] \
    || printf '%s\n' "warning: $slug's permissions differ from the manifest's; check them in the App's settings" >&2
  [ "$events_ok" = true ] \
    || printf '%s\n' "warning: $slug's events differ from the manifest's; check them in the App's settings" >&2

  lookup_bot_id "$slug"

  store="$("$MSW_HERE/key-store.sh" backend "$service" "$client_id")" || exit 1
  import_key "$service" "$client_id" "$pem" "$store" "$label"
  pem=""

  mkdir -p "$(dirname "$cfg")"
  # noclobber: the config was checked to be absent before the flow started.
  ( set -C
    jq -n --arg id "$id" --arg slug "$slug" --arg client_id "$client_id" \
          --arg keychain_service "$service" --arg key_store "$store" \
          --arg fingerprint "SHA256:$KEY_FINGERPRINT" --arg key_confirm none --arg bot_user_id "$BOT_USER_ID" \
          '{app_id: ($id|tonumber), slug: $slug, client_id: $client_id, keychain_service: $keychain_service, key_store: $key_store, key_fingerprint: $fingerprint, bot_user_id: ($bot_user_id|tonumber), key_confirm: $key_confirm}' \
      > "$cfg"
  ) || msw_die "cannot write $cfg"
  PENDING=""

  printf '%s\n' "created: app $slug (id $id, client $client_id, bot user $BOT_USER_ID)"
  printf '%s\n' "key in the $store key store, confirmation: none"
  printf '%s\n' "key fingerprint SHA256:$KEY_FINGERPRINT (compare with the App's settings page)"
  printf '%s\n' "identifiers: $cfg"
  case "$role" in
    keeper)
      printf '%s\n' "next: the maintainer's admin steps in $MSW_HERE/README.md, Keeper (install it; the bypass on master)" ;;
    *)
      printf '%s\n' "next: install the App ($MSW_HERE/README.md, Setup)" ;;
  esac
}

# --- bot-id ------------------------------------------------------------------

bot_id() {
  local slug updated
  slug="$(msw_config_slug "$MSW_APP_CONFIG")" || exit 1
  lookup_bot_id "$slug"
  updated="$(jq --arg id "$BOT_USER_ID" '.bot_user_id = ($id|tonumber)' "$MSW_APP_CONFIG")" \
    || msw_die "cannot read $MSW_APP_CONFIG"
  printf '%s\n' "$updated" > "$MSW_APP_CONFIG"
  printf '%s\n' "recorded: ${slug}[bot] is bot user $BOT_USER_ID in $MSW_APP_CONFIG"
}

# --- rotate ------------------------------------------------------------------

rotate() {
  local path="$1" keep="$2" pem service client_id slug confirm updated store old label
  [ -f "$path" ] || msw_die "no such file: $path"
  [ -f "$MSW_APP_CONFIG" ] || msw_die "no App config at $MSW_APP_CONFIG: rotate needs an existing App"
  slug="$(msw_config_slug "$MSW_APP_CONFIG")" \
    || msw_die "$MSW_APP_CONFIG is no role's App config (roles: bee, keeper; see README.md, Roles)"
  label="$(key_label "$slug")"
  service="$(jq -r '.keychain_service // empty | strings' "$MSW_APP_CONFIG")"
  check_word "keychain_service in $MSW_APP_CONFIG" "$service"
  client_id="$(jq -r '.client_id // empty' "$MSW_APP_CONFIG")"
  confirm="$(jq -r '.key_confirm // empty' "$MSW_APP_CONFIG")"
  case "$confirm" in
    ''|none) ;;
    *) msw_die "$MSW_APP_CONFIG records key_confirm '$confirm', but no App's key needs per-use confirmation any more: delete the key-store item (key-store.sh delete $service $client_id) and set key_confirm to none in $MSW_APP_CONFIG, then rotate again" ;;
  esac
  old="$(msw_recorded_key_store 2>/dev/null)" || old=""
  store="${MSW_KEY_STORE:-}"
  if [ -z "$store" ]; then
    store="$(msw_recorded_key_store)" || exit 1
  fi
  pem="$(cat "$path")"

  import_key "$service" "$client_id" "$pem" "$store" "$label"
  pem=""

  updated="$(jq --arg fp "SHA256:$KEY_FINGERPRINT" --arg ks "$store" --arg kc none \
    '.key_fingerprint = $fp | .key_store = $ks | .key_confirm = $kc' "$MSW_APP_CONFIG")"
  printf '%s\n' "$updated" > "$MSW_APP_CONFIG"
  printf '%s\n' "key stored in the $store key store, confirmation: none"
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
  create)
    [ $# -eq 3 ] || usage
    trap on_exit EXIT
    create "$2" "$3" ;;
  bot-id)
    [ $# -eq 1 ] || usage
    bot_id ;;
  rotate)
    case "$#:${3:-}" in
      2:) rotate "$2" 0 ;;
      3:--keep) rotate "$2" 1 ;;
      *) usage ;;
    esac ;;
  *)
    usage ;;
esac
