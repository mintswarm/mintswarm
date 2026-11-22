#!/usr/bin/env bash
# Keep a secret, such as the App's private key, in a key store, with the value on
# stdin and stdout only.
#
#   key-store.sh backend [SERVICE ACCOUNT]    print the backend put would use
#   key-store.sh put SERVICE ACCOUNT [LABEL]  store the value read from stdin
#   key-store.sh get SERVICE ACCOUNT          print the value, byte for byte, to a
#                                             pipe or file; refused to a terminal
#   key-store.sh delete SERVICE ACCOUNT       remove it
#
# The backend is $MSW_KEY_STORE when set; the command fails if it is unknown or
# unavailable here. Otherwise it is the first available of:
#   keychain        uname -s is Darwin and security is on PATH
#   secret-service  secret-tool is on PATH and there is a D-Bus session bus
#                   (DBUS_SESSION_BUS_ADDRESS, or a socket at $XDG_RUNTIME_DIR/bus)
#   pass            pass is on PATH and the store is initialised
#                   (${PASSWORD_STORE_DIR:-~/.password-store}/.gpg-id)
#   systemd-creds   only when SERVICE and ACCOUNT are given and
#                   $CREDENTIALS_DIRECTORY/SERVICE@ACCOUNT is a readable file
# and fails, listing every MSW_KEY_STORE value, when none is. file is never
# picked automatically. No availability test runs a backend command, so none can
# unlock a keyring or prompt; the one exception is systemd-creds --version, run
# only for an explicit MSW_KEY_STORE=systemd-creds as a non-root user.
#
# Where each backend keeps the value:
#   keychain        a generic password -s SERVICE -a ACCOUNT -l LABEL in the login
#                   Keychain, base64 on the stdin of "security -i"
#   secret-service  an item with attributes service=SERVICE account=ACCOUNT,
#                   written on the stdin of "secret-tool store". Every secret-tool
#                   call is stopped after $MSW_KEY_STORE_TIMEOUT seconds (default
#                   10): on a locked collection it waits for an unlock prompt,
#                   which an agent session never answers.
#   pass            the entry mintswarm/SERVICE/ACCOUNT, written on the stdin of
#                   "pass insert -m -f"
#   systemd-creds   the credential SERVICE@ACCOUNT, encrypted from stdin into
#                   ~/.config/mintswarm/credstore/SERVICE@ACCOUNT.cred, with
#                   --user unless root (which then needs systemd 256 or later);
#                   get reads $CREDENTIALS_DIRECTORY/SERVICE@ACCOUNT first
#   file            ~/.config/mintswarm/keys/SERVICE@ACCOUNT, unencrypted, 0600 in
#                   a 0700 directory; refused when either is a symlink, is not the
#                   user's, or has any group or other permission, when ~/.config
#                   or ~/.config/mintswarm is a symlink (a dotfiles link), and
#                   when the directory is inside a git work tree. Every use warns.
#
# SERVICE and ACCOUNT match [A-Za-z0-9][A-Za-z0-9._-]*; LABEL matches
# [A-Za-z0-9 ._-]+ and defaults to "mintswarm SERVICE ACCOUNT".
#
# The value is never in argv or a temporary file: it moves only through pipes,
# shell variables and the printf builtin. An empty value is refused. get refuses
# to write to a terminal, so the key is not shown by accident; it does not stop a
# pipe into cat, so never run get for the App's key in a recorded session.
#
# Exit status: 0 on success; 1 on failure (not found, unavailable, refused);
# 2 on bad usage.
#
# bash 3.2 compatible.
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

umask 077

usage() {
  printf '%s\n' "usage: key-store.sh backend [SERVICE ACCOUNT] | put SERVICE ACCOUNT [LABEL] | get SERVICE ACCOUNT | delete SERVICE ACCOUNT" >&2
  exit 2
}
bad() { printf 'key-store.sh: %s\n' "$*" >&2; exit 2; }

timeout_secs="${MSW_KEY_STORE_TIMEOUT:-10}"
case "$timeout_secs" in
  ''|*[!0-9]*|0) bad "MSW_KEY_STORE_TIMEOUT must be a whole number of seconds above 0" ;;
esac
say() { printf 'key-store.sh: %s\n' "$*" >&2; }

check_name() {  # check_name WHAT VALUE
  case "$2" in
    [A-Za-z0-9]*) ;;
    *) bad "invalid $1: '$2'" ;;
  esac
  case "$2" in
    *[!A-Za-z0-9._-]*) bad "invalid $1: '$2'" ;;
  esac
}

check_label() {
  case "$1" in
    ''|*[!A-Za-z0-9\ ._-]*) bad "invalid label: '$1'" ;;
  esac
}

have() { command -v "$1" >/dev/null 2>&1; }
is_root() { [ "$(id -u)" = 0 ]; }

# --- availability ------------------------------------------------------------

sdc_passed() {  # sdc_passed [SERVICE ACCOUNT]: $CREDENTIALS_DIRECTORY holds the credential
  [ $# -eq 2 ] && [ -n "${CREDENTIALS_DIRECTORY:-}" ] \
    && [ -f "$CREDENTIALS_DIRECTORY/$1@$2" ] && [ -r "$CREDENTIALS_DIRECTORY/$1@$2" ]
}

sdc_usable() {  # systemd-creds can encrypt and decrypt for this user
  local v
  have systemd-creds || return 1
  is_root && return 0
  v="$(systemd-creds --version 2>/dev/null | sed -n '1s/^systemd \([0-9][0-9]*\).*/\1/p')" || return 1
  [ -n "$v" ] && [ "$v" -ge 256 ]
}

available() {  # available BACKEND [SERVICE ACCOUNT]
  local b="$1"; shift
  case "$b" in
    keychain) [ "$(uname -s 2>/dev/null)" = Darwin ] && have security ;;
    secret-service)
      have secret-tool \
        && { [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ] || [ -S "${XDG_RUNTIME_DIR:-/nonexistent}/bus" ]; } ;;
    pass) have pass && [ -f "${PASSWORD_STORE_DIR:-$HOME/.password-store}/.gpg-id" ] ;;
    systemd-creds) sdc_passed "$@" || sdc_usable ;;
    file) return 0 ;;
    *) return 1 ;;
  esac
}

needs() {  # needs BACKEND: what it needs, for the error message
  case "$1" in
    keychain) printf '%s' "macOS, with security on PATH" ;;
    secret-service) printf '%s' "secret-tool on PATH and a D-Bus session bus (DBUS_SESSION_BUS_ADDRESS or \$XDG_RUNTIME_DIR/bus)" ;;
    pass) printf '%s' "pass on PATH and an initialised store (${PASSWORD_STORE_DIR:-$HOME/.password-store}/.gpg-id)" ;;
    systemd-creds) printf '%s' "systemd-creds on PATH (systemd 256 or later unless root), or the credential in \$CREDENTIALS_DIRECTORY" ;;
  esac
}

store=""
select_backend() {  # select_backend [SERVICE ACCOUNT]: sets store
  local b
  if [ -n "${MSW_KEY_STORE:-}" ]; then
    case "$MSW_KEY_STORE" in
      keychain|secret-service|pass|systemd-creds|file) ;;
      *) msw_die "unknown MSW_KEY_STORE '$MSW_KEY_STORE': use one of $MSW_KEY_STORES" ;;
    esac
    available "$MSW_KEY_STORE" "$@" \
      || msw_die "MSW_KEY_STORE=$MSW_KEY_STORE is not available here: it needs $(needs "$MSW_KEY_STORE")"
    store="$MSW_KEY_STORE"
    return 0
  fi
  for b in keychain secret-service pass; do
    if available "$b"; then store="$b"; return 0; fi
  done
  if sdc_passed "$@"; then store=systemd-creds; return 0; fi
  msw_die "no key store found here: set MSW_KEY_STORE to one of $MSW_KEY_STORES (file keeps the key unencrypted and is never picked automatically; see README.md, Key store)"
}

# --- backends ----------------------------------------------------------------
# Each NAME_put SERVICE ACCOUNT LABEL reads $value; each NAME_get SERVICE ACCOUNT
# writes the value to stdout; each NAME_delete SERVICE ACCOUNT removes it. They
# return non-zero on failure.

keychain_put() {
  local b64
  b64="$(printf '%s' "$value" | openssl base64 -A)" || return 1
  printf 'add-generic-password -U -s %s -a %s -l "%s" -w %s\n' "$1" "$2" "$3" "$b64" | security -i >/dev/null
}
keychain_get() { security find-generic-password -s "$1" -a "$2" -w | openssl base64 -d -A; }
keychain_delete() { security delete-generic-password -s "$1" -a "$2" >/dev/null; }

bounded() {  # bounded COMMAND...: COMMAND, stopped after $timeout_secs seconds; 124 when stopped
  # A watchdog, not timeout(1), so the same code runs on macOS. COMMAND keeps this
  # stdin (0<&0: an asynchronous command would otherwise get /dev/null) and
  # stdout; the watchdog holds neither, and takes its sleep with it when stopped.
  local pid watchdog rc=0
  "$@" 0<&0 &
  pid=$!
  (
    trap 'kill "$s" 2>/dev/null; exit 0' TERM
    sleep "$timeout_secs" &
    s=$!
    wait "$s" && kill "$pid" 2>/dev/null
  ) </dev/null >/dev/null 2>&1 &
  watchdog=$!
  wait "$pid" || rc=$?
  kill "$watchdog" 2>/dev/null || true
  wait "$watchdog" 2>/dev/null || true
  if [ "$rc" -eq 143 ]; then return 124; fi
  return "$rc"
}
secret_tool() {  # secret_tool ARGS...: secret-tool, bounded, with a clear message when it does not answer
  local rc=0
  bounded secret-tool "$@" || rc=$?
  if [ "$rc" -eq 124 ]; then
    say "secret-tool did not answer within $timeout_secs seconds: the keyring is probably locked; unlock it first (see README.md, Key store)"
  fi
  return "$rc"
}
secret_service_put() { printf '%s' "$value" | secret_tool store "--label=$3" service "$1" account "$2"; }
secret_service_get() { secret_tool lookup service "$1" account "$2"; }
secret_service_delete() { secret_tool clear service "$1" account "$2"; }

pass_put() { printf '%s' "$value" | pass insert -m -f "mintswarm/$1/$2" >/dev/null; }
pass_get() { pass show "mintswarm/$1/$2"; }
pass_delete() { pass rm -f "mintswarm/$1/$2" >/dev/null; }

sdc() { if is_root; then systemd-creds "$@"; else systemd-creds --user "$@"; fi; }
sdc_require() {
  sdc_usable && return 0
  say "systemd-creds cannot encrypt or decrypt here: it needs systemd-creds on PATH, and systemd 256 or later unless root"
  return 1
}
systemd_creds_put() {
  sdc_require || return 1
  mkdir -p "$MSW_CREDSTORE_DIR" || return 1
  printf '%s' "$value" | sdc encrypt "--name=$1@$2" - "$MSW_CREDSTORE_DIR/$1@$2.cred"
}
systemd_creds_get() {
  local f="$MSW_CREDSTORE_DIR/$1@$2.cred"
  if sdc_passed "$1" "$2"; then
    cat "$CREDENTIALS_DIRECTORY/$1@$2"
    return
  fi
  [ -f "$f" ] || { say "no credential $1@$2 in \$CREDENTIALS_DIRECTORY or $f"; return 1; }
  sdc_require || return 1
  sdc decrypt "--name=$1@$2" "$f" -
}
systemd_creds_delete() {
  local f="$MSW_CREDSTORE_DIR/$1@$2.cred"
  [ -f "$f" ] || { say "no credential file $f"; return 1; }
  rm -f "$f"
}

file_wide() {  # file_wide PATH: PATH has any group or other permission bit
  [ -n "$(find "$1" -prune \( -perm -0040 -o -perm -0020 -o -perm -0010 -o -perm -0004 -o -perm -0002 -o -perm -0001 \) -print)" ]
}
file_check() {  # file_check WHAT PATH: refuse a symlink, another owner or a wide mode
  if [ -L "$2" ]; then say "refusing the $1 $2: it is a symlink"; return 1; fi
  if [ "$1" = directory ] && [ ! -d "$2" ]; then say "refusing $2: not a directory"; return 1; fi
  if [ "$1" = file ] && [ ! -f "$2" ]; then say "refusing $2: not a regular file"; return 1; fi
  if [ ! -O "$2" ]; then say "refusing the $1 $2: it is not owned by $(id -un)"; return 1; fi
  if file_wide "$2"; then say "refusing the $1 $2: it has group or other permissions (chmod go= it)"; return 1; fi
}
file_warn() { say "warning: the file key store keeps the key unencrypted in $1"; }
file_parents() {  # refuse a symlink between $HOME and the key directory, or a key directory in a git work tree
  local p
  for p in "$HOME/.config" "$MSW_CONFIG_DIR"; do
    if [ -L "$p" ]; then
      say "refusing $p: it is a symlink (a dotfiles link?); the file key store needs real directories below \$HOME"
      return 1
    fi
  done
  p="$MSW_KEY_FILE_DIR"
  while [ ! -d "$p" ]; do p="$(dirname "$p")"; done
  if have git && [ "$(cd "$p" && env -u GIT_DIR -u GIT_WORK_TREE -u GIT_CEILING_DIRECTORIES \
       git rev-parse --is-inside-work-tree 2>/dev/null)" = true ]; then
    say "refusing $MSW_KEY_FILE_DIR: it is inside a git work tree, where the key could be committed"
    return 1
  fi
}
file_put() {
  local d="$MSW_KEY_FILE_DIR" f="$MSW_KEY_FILE_DIR/$1@$2"
  file_warn "$f"
  file_parents || return 1
  if [ ! -e "$d" ] && [ ! -L "$d" ]; then mkdir -p "$d" || return 1; fi
  file_check directory "$d" || return 1
  if [ -e "$f" ] || [ -L "$f" ]; then file_check file "$f" || return 1; fi
  printf '%s' "$value" > "$f"
}
file_get() {
  local d="$MSW_KEY_FILE_DIR" f="$MSW_KEY_FILE_DIR/$1@$2"
  file_warn "$f"
  file_parents || return 1
  if [ ! -e "$d" ] && [ ! -L "$d" ]; then say "no $d"; return 1; fi
  file_check directory "$d" || return 1
  if [ ! -e "$f" ] && [ ! -L "$f" ]; then say "no $f"; return 1; fi
  file_check file "$f" || return 1
  cat "$f"
}
file_delete() {
  local d="$MSW_KEY_FILE_DIR" f="$MSW_KEY_FILE_DIR/$1@$2"
  file_warn "$f"
  if [ ! -e "$d" ] && [ ! -L "$d" ]; then say "no $d"; return 1; fi
  file_check directory "$d" || return 1
  if [ ! -e "$f" ] && [ ! -L "$f" ]; then say "no $f"; return 1; fi
  rm -f "$f"
}

backend_fn() {  # backend_fn BACKEND OP: the function name
  case "$1" in
    secret-service) printf 'secret_service_%s' "$2" ;;
    systemd-creds) printf 'systemd_creds_%s' "$2" ;;
    *) printf '%s_%s' "$1" "$2" ;;
  esac
}

# --- main --------------------------------------------------------------------

value=""
case "${1:-}" in
  backend)
    case $# in
      1) ;;
      3) check_name service "$2"; check_name account "$3" ;;
      *) usage ;;
    esac
    shift
    select_backend "$@"
    printf '%s\n' "$store" ;;
  put)
    [ $# -eq 3 ] || [ $# -eq 4 ] || usage
    check_name service "$2"; check_name account "$3"
    label="${4:-mintswarm $2 $3}"
    check_label "$label"
    select_backend "$2" "$3"
    value="$(cat && printf x)" || msw_die "cannot read the value on stdin"
    value="${value%x}"
    [ -n "$value" ] || msw_die "refusing an empty value"
    "$(backend_fn "$store" put)" "$2" "$3" "$label" \
      || msw_die "storing $2/$3 in the $store key store failed"
    value="" ;;
  get)
    [ $# -eq 3 ] || usage
    check_name service "$2"; check_name account "$3"
    [ ! -t 1 ] || msw_die "refusing to print $2/$3 to a terminal: pipe it into the command that needs it"
    select_backend "$2" "$3"
    value="$("$(backend_fn "$store" get)" "$2" "$3" && printf x)" \
      || msw_die "reading $2/$3 from the $store key store failed"
    value="${value%x}"
    [ -n "$value" ] || msw_die "$2/$3 in the $store key store is empty"
    printf '%s' "$value"
    value="" ;;
  delete)
    [ $# -eq 3 ] || usage
    check_name service "$2"; check_name account "$3"
    select_backend "$2" "$3"
    "$(backend_fn "$store" delete)" "$2" "$3" \
      || msw_die "deleting $2/$3 from the $store key store failed" ;;
  *)
    usage ;;
esac
