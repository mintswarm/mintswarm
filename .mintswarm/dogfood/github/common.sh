# shellcheck shell=bash
# shellcheck disable=SC2034  # the constants are used by the scripts that source this file
# Shared constants and helpers for the mintswarm-dev tooling. Sourced, never run.
#
# Contract:
#   - The repository, bot identity and config locations are defined here and
#     nowhere else in .mintswarm/dogfood/github/.
#   - msw_token [PERMS_JSON] prints an installation token: $MSW_TOKEN when set,
#     otherwise a fresh one from mint-token.sh. It fails unless the token starts
#     with ghs_. Capture it with tok="$(msw_token)" || exit; never print it.
#   - msw_git_env ARGS... runs git with only the repository's own config: no
#     system or global config, no config from the environment (including the
#     legacy GIT_CONFIG, which changes what "git config" reads but not what
#     "git push" reads), no TLS overrides from the environment
#     (GIT_SSL_NO_VERIFY, GIT_SSL_CAINFO, GIT_SSL_CAPATH), no askpass, no
#     prompts and no tracing.
#   - msw_git_net ARGS... is msw_git_env plus a credential helper list reset to
#     a single inline helper that answers with $MSW_TOKEN from the environment,
#     and only for a request with protocol=https and host=github.com; no hooks;
#     and transports limited to https (and file, only when the caller sets
#     MSW_GIT_ALLOW_FILE=1 itself; the environment cannot), so no redirect can
#     reach ssh and the maintainer's key.
#     The token is never in a URL, a file, a credential store or any argv.
#   - msw_recorded_key_store prints the key store recorded as key_store in
#     $MSW_APP_CONFIG; a file without it means keychain on macOS and is an
#     error elsewhere. mint-token.sh reads the key only from that store.
#   - msw_app_profile NAME is the per-App table: for each App that
#     store-credentials.sh creates from a manifest, it sets
#       MSW_PROFILE_CONFIG   the App's own config file
#       MSW_PROFILE_SERVICE  the key's service name in every key store
#       MSW_PROFILE_LABEL    the key's label
#     for mintswarm-dev ($MSW_APP_CONFIG) and mintswarm-dev-steward
#     ($MSW_STEWARD_CONFIG, never derived from $MSW_APP_CONFIG), and returns 1
#     for any other name.
#
# bash 3.2 compatible.

MSW_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The repository the helpers act on, as owner/name; mintswarm/mintswarm unless
# MSW_REPO names another repository the App is installed on.
MSW_REPO="${MSW_REPO:-mintswarm/mintswarm}"
if ! [[ "$MSW_REPO" =~ ^[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_-][A-Za-z0-9._-]*$ ]]; then
  printf '%s: MSW_REPO must be owner/name, not %s\n' "$(basename "$0")" "$MSW_REPO" >&2
  exit 1
fi
MSW_REPO_OWNER="${MSW_REPO%%/*}"
MSW_REPO_NAME="${MSW_REPO#*/}"
MSW_REPO_URL="https://github.com/$MSW_REPO.git"

MSW_BOT_NAME='mintswarm-dev[bot]'
MSW_BOT_EMAIL='359856778+mintswarm-dev[bot]@users.noreply.github.com'

MSW_BRANCH_PREFIX=mintswarm/

MSW_KEYCHAIN_SERVICE=mintswarm-dev-private-key
MSW_CONFIG_DIR="$HOME/.config/mintswarm"
MSW_APP_CONFIG="${MSW_APP_CONFIG:-$MSW_CONFIG_DIR/dev-app.json}"
MSW_STEWARD_KEYCHAIN_SERVICE=mintswarm-dev-steward-private-key
MSW_STEWARD_CONFIG="${MSW_STEWARD_CONFIG:-$MSW_CONFIG_DIR/steward-app.json}"
MSW_KEY_FILE_DIR="$MSW_CONFIG_DIR/keys"
MSW_CREDSTORE_DIR="$MSW_CONFIG_DIR/credstore"
MSW_KEY_STORES="keychain, secret-service, pass, systemd-creds, file"

# Set by the sourcing script, never taken from the environment: only push.sh sets
# it, and only for a file:// MSW_PUSH_URL (selftest.sh).
MSW_GIT_ALLOW_FILE=0

msw_die() {
  printf '%s: %s\n' "$(basename "$0")" "$*" >&2
  exit 1
}

msw_token() {
  local perms="${1:-}" tok
  if [ -n "${MSW_TOKEN:-}" ]; then
    tok="$MSW_TOKEN"
  else
    tok="$("$MSW_HERE/mint-token.sh" token "$perms")" || {
      printf '%s\n' "msw_token: minting an installation token failed" >&2
      return 1
    }
  fi
  case "$tok" in
    ghs_?*) ;;
    *)
      printf '%s\n' "msw_token: not an installation token (expected ghs_...)" >&2
      return 1 ;;
  esac
  printf '%s' "$tok"
}

msw_recorded_key_store() {
  local store
  store="$(jq -r '.key_store // empty' "$MSW_APP_CONFIG")" || msw_die "cannot read $MSW_APP_CONFIG"
  if [ -z "$store" ]; then
    [ "$(uname -s)" = Darwin ] \
      || msw_die "key_store missing from $MSW_APP_CONFIG: store the key here with MSW_KEY_STORE=<one of: $MSW_KEY_STORES> store-credentials.sh rotate PEM_PATH (see README.md, Key store)"
    store=keychain
  fi
  printf '%s\n' "$store"
}

MSW_PROFILE_CONFIG=""
MSW_PROFILE_SERVICE=""
MSW_PROFILE_LABEL=""
msw_app_profile() {  # msw_app_profile NAME: set MSW_PROFILE_*; 1 for an unknown NAME
  case "$1" in
    mintswarm-dev)
      MSW_PROFILE_CONFIG="$MSW_APP_CONFIG"
      MSW_PROFILE_SERVICE="$MSW_KEYCHAIN_SERVICE"
      MSW_PROFILE_LABEL='mintswarm-dev GitHub App private key' ;;
    mintswarm-dev-steward)
      MSW_PROFILE_CONFIG="$MSW_STEWARD_CONFIG"
      MSW_PROFILE_SERVICE="$MSW_STEWARD_KEYCHAIN_SERVICE"
      MSW_PROFILE_LABEL='mintswarm-dev-steward GitHub App private key' ;;
    *)
      MSW_PROFILE_CONFIG=""
      MSW_PROFILE_SERVICE=""
      MSW_PROFILE_LABEL=""
      return 1 ;;
  esac
}

msw_git_env() {
  env -u GIT_ASKPASS -u SSH_ASKPASS \
      -u GIT_TRACE -u GIT_TRACE_CURL -u GIT_TRACE_PACKET -u GIT_CURL_VERBOSE -u GIT_TRACE_REDACT \
      -u GIT_CONFIG_PARAMETERS -u GIT_CONFIG_COUNT -u GIT_CONFIG \
      -u GIT_SSL_NO_VERIFY -u GIT_SSL_CAINFO -u GIT_SSL_CAPATH \
      GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 \
      git "$@"
}

msw_git_net() {
  local protocols=https file=never
  if [ "$MSW_GIT_ALLOW_FILE" = 1 ]; then
    protocols=https:file
    file=always
  fi
  # GIT_ALLOW_PROTOCOL overrides any protocol.* config; the -c keys say the same.
  # The helper is single-quoted on purpose: git's shell expands $MSW_TOKEN from
  # the environment, so the token never appears in this argv. It reads the
  # request up to the blank line and answers only for https://github.com, so a
  # planted redirect to another https host never receives the token.
  # shellcheck disable=SC2016
  GIT_ALLOW_PROTOCOL="$protocols" msw_git_env \
    -c protocol.allow=never \
    -c protocol.https.allow=always \
    -c "protocol.file.allow=$file" \
    -c core.hooksPath=/dev/null \
    -c credential.helper= \
    -c 'credential.helper=!f() { test "$1" = get || exit 0; p=; h=; while IFS= read -r l; do case "$l" in "") break;; protocol=https) p=1;; host=github.com) h=1;; esac; done; test "$p$h" = 11 && printf "username=x-access-token\npassword=%s\n" "$MSW_TOKEN"; }; f' \
    -c core.askPass= \
    "$@"
}
