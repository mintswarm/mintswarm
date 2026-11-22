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
#     system or global config, no config from the environment, no askpass, no
#     prompts and no tracing.
#   - msw_git_net ARGS... is msw_git_env plus a credential helper list reset to
#     a single inline helper that answers with $MSW_TOKEN from the environment,
#     no hooks, and transports limited to https (and file, only when the caller
#     sets MSW_GIT_ALLOW_FILE=1 itself; the environment cannot), so no redirect
#     can reach ssh and the maintainer's key.
#     The token is never in a URL, a file, a credential store or any argv.
#
# bash 3.2 compatible.

MSW_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MSW_REPO_OWNER=mintswarm
MSW_REPO_NAME=mintswarm
MSW_REPO_URL=https://github.com/mintswarm/mintswarm.git

MSW_BOT_NAME='mintswarm-dev[bot]'
MSW_BOT_EMAIL='359856778+mintswarm-dev[bot]@users.noreply.github.com'

MSW_BRANCH_PREFIX=mintswarm/

MSW_KEYCHAIN_SERVICE=mintswarm-dev-private-key
MSW_CONFIG_DIR="$HOME/.config/mintswarm"
MSW_APP_CONFIG="${MSW_APP_CONFIG:-$MSW_CONFIG_DIR/dev-app.json}"

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

msw_git_env() {
  env -u GIT_ASKPASS -u SSH_ASKPASS \
      -u GIT_TRACE -u GIT_TRACE_CURL -u GIT_TRACE_PACKET -u GIT_CURL_VERBOSE -u GIT_TRACE_REDACT \
      -u GIT_CONFIG_PARAMETERS -u GIT_CONFIG_COUNT \
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
  # the environment, so the token never appears in this argv.
  # shellcheck disable=SC2016
  GIT_ALLOW_PROTOCOL="$protocols" msw_git_env \
    -c protocol.allow=never \
    -c protocol.https.allow=always \
    -c "protocol.file.allow=$file" \
    -c core.hooksPath=/dev/null \
    -c credential.helper= \
    -c 'credential.helper=!f() { test "$1" = get && printf "username=x-access-token\npassword=%s\n" "$MSW_TOKEN"; }; f' \
    -c core.askPass= \
    "$@"
}
