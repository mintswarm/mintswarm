#!/usr/bin/env bash
# The Linux check, run as root inside the container that linux-check.sh starts.
# It refuses to run anywhere else, in every mode: it moves the systemd host
# credential key aside, kills the msw user's processes and runs commands as
# another user. It needs the marker /etc/mintswarm-linux-check that only
# linux/Dockerfile writes, besides /.dockerenv or /run/.containerenv; the user-*
# modes also need to be msw with HOME=/home/msw, the others root.
#
#   check.sh                 run every phase (as root)
#   check.sh PHASE [STEP]    one phase's steps, as the driver starts them:
#                            user-a, user-b, user-c and user-d STEP as msw,
#                            root-e as root
#
# Phases, each under a timeout:
#   A  selftest.sh as the non-root user msw: GNU userland, bash 5, Debian's tools.
#   B  secret-service as msw, against gnome-keyring in dbus-run-session: backend
#      selection, a byte-exact round trip, rotate and mint-token.sh jwt; with no
#      bus, get fails; with the collection locked, get fails within 20 seconds
#      and says to unlock it; nothing falls back to the file backend.
#   C  pass as msw, with a throwaway gpg key: round trip, rotate and jwt.
#   D  the file backend as msw: modes 600 and 700, the warning, refusing a
#      symlink, a wide mode and (after root chowns it) a foreign owner; rotate and jwt.
#   E  systemd-creds as root with the host key: round trip, rotate and jwt; with
#      the host key moved aside get fails, so the null key was not used.
#   F  as root: no traced argv of B to E (strace, execve) holds a body line of a
#      throwaway key or PRIVATE KEY; every log shows its backend's binary run;
#      every TMPDIR is empty; no file under /home /root /tmp /run /dev/shm /var
#      holds a key line, except the file backend's own key files.
#
# Prints the package versions, one line per check (ok, FAIL or skip) and the
# totals; exits 0 only if every check passed.
#
# shellcheck disable=SC2016
# SC2016: sh -c '...' scripts take their values as positional parameters, and
# dpkg-query expands its own ${Package} and ${Version}.
set -euo pipefail

G="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # .mintswarm/dogfood/github
SELF="$G/linux/check.sh"
W=/srv/msw-check                                       # throwaway keys and traces; not searched in F
SERVICE=mintswarm-dev-private-key
CLIENT_ID=Iv23liLinuxCheck01
PATH_SAFE=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
MSW_HOME=/home/msw
MSW_RUN=/run/user/1000

passed=0
failed=0
skipped=0
ok() { passed=$((passed + 1)); printf 'ok    %s\n' "$1"; }
not_ok() { failed=$((failed + 1)); printf 'FAIL  %s\n' "$1"; }
skip() { skipped=$((skipped + 1)); printf 'skip  %s\n' "$1"; }
check() {  # check NAME COMMAND...: COMMAND succeeds; its output is shown only on failure
  local name="$1"; shift
  if "$@" >"$HOME/check.out" 2>&1; then
    ok "$name"
  else
    not_ok "$name"; sed 's/^/      /' "$HOME/check.out"
  fi
}
refuses() {  # refuses NAME COMMAND...: COMMAND fails
  local name="$1"; shift
  if "$@" >"$HOME/check.out" 2>&1; then
    not_ok "$name"; sed 's/^/      /' "$HOME/check.out"
  else
    ok "$name"
  fi
}
eq() { [ "$1" = "$2" ] || { printf '      expected: %s\n      got:      %s\n' "$2" "$1"; return 1; }; }
fails_saying() {  # fails_saying TEXT COMMAND...: COMMAND fails and its output contains TEXT
  local text="$1"; shift
  if "$@" >"$HOME/fails.out" 2>&1; then echo "succeeded"; cat "$HOME/fails.out"; return 1; fi
  cat "$HOME/fails.out"
  grep -qF -- "$text" "$HOME/fails.out"
}
phase_done() { printf 'phase-done %s\n' "$1"; }

K="$G/key-store.sh"
S="$G/store-credentials.sh"
M="$G/mint-token.sh"

b64url_decode() {
  local s
  s="$(printf '%s' "$1" | tr '_-' '/+')"
  while [ $((${#s} % 4)) -ne 0 ]; do s="$s="; done
  printf '%s' "$s" | openssl base64 -d -A
}
verify_jwt() {  # verify_jwt PUBKEY: mint-token.sh jwt gives a JWT that verifies with PUBKEY
  local jwt h p s d="$HOME/jwt"
  jwt="$("$M" jwt)" || return 1
  h="${jwt%%.*}"; s="${jwt##*.}"; p="${jwt#*.}"; p="${p%.*}"
  mkdir -p "$d"
  printf '%s.%s' "$h" "$p" > "$d/input"
  b64url_decode "$s" > "$d/sig"
  openssl dgst -sha256 -verify "$1" -signature "$d/sig" "$d/input" >/dev/null || return 1
  b64url_decode "$p" > "$d/payload"
  eq "$(jq -r .iss "$d/payload")" "$CLIENT_ID"
}
new_app_config() {  # new_app_config NAME: a dev-app.json without key_store, as copied from macOS
  MSW_APP_CONFIG="$HOME/.config/mintswarm/dev-app.$1.json"
  export MSW_APP_CONFIG
  mkdir -p "$HOME/.config/mintswarm"
  jq -n --arg client_id "$CLIENT_ID" --arg service "$SERVICE" \
    '{app_id: 1, slug: "mintswarm-dev", client_id: $client_id, keychain_service: $service, key_fingerprint: "SHA256:unset"}' \
    > "$MSW_APP_CONFIG"
}
# Values read back are compared on a pipe, never saved, so phase F finds no copy.
same_as() { "$K" get msw-check "$2" | cmp "$1" -; }   # same_as PEM ACCOUNT
roundtrip() {  # roundtrip ACCOUNT: put key1, get it back byte for byte, delete, get fails
  "$K" put msw-check "$1" < "$W/key1.pem" || return 1
  same_as "$W/key1.pem" "$1" || return 1
  "$K" delete msw-check "$1" || return 1
  if "$K" get msw-check "$1" > /dev/null; then echo "get after delete succeeded"; return 1; fi
}
rotate_then_jwt() {  # rotate_then_jwt BACKEND: rotate key1 into BACKEND, then sign a JWT with it
  new_app_config "$1"
  MSW_KEY_STORE="$1" "$S" rotate "$W/key1.pem" --keep || return 1
  eq "$(jq -r .key_store "$MSW_APP_CONFIG")" "$1" || return 1
  ( unset MSW_KEY_STORE; verify_jwt "$W/pub1.pem" )
}
no_key_dir() { test ! -e "$HOME/.config/mintswarm/keys"; }

# --- A: selftest.sh, as msw -----------------------------------------------------

user_a() {
  "$G/selftest.sh"
  phase_done A
}

# --- B: secret-service, as msw inside dbus-run-session -------------------------

kill_strays() {  # kill msw's processes outside this script's ancestry (the keyring daemon, any prompter);
                 # dbus-daemon is left to dbus-run-session, which stops it when this script exits
  local keep=" " p="$$" stat q
  while [ "$p" -gt 1 ]; do
    keep="$keep$p "
    stat="$(cat "/proc/$p/stat" 2>/dev/null)" || break
    stat="${stat##*) }"; stat="${stat#* }"; p="${stat%% *}"
  done
  for q in /proc/[0-9]*; do
    q="${q#/proc/}"
    case "$keep" in *" $q "*) continue ;; esac
    [ "$(cat "/proc/$q/comm" 2>/dev/null || true)" != dbus-daemon ] || continue
    if [ -O "/proc/$q" ]; then kill "$q" 2>/dev/null || true; fi
  done
}

locked_get() {  # with the collection locked: get fails within 20 seconds and says to unlock it
  local start end
  start="$(date +%s)"
  fails_saying "the keyring is probably locked; unlock it first" timeout 60 "$K" get "$SERVICE" "$CLIENT_ID" || return 1
  end="$(date +%s)"
  [ $((end - start)) -lt 20 ] || { echo "took $((end - start)) seconds"; return 1; }
}

user_b() {
  # Nothing this phase starts may outlive it: strace -f waits for every process it traces.
  trap kill_strays EXIT
  unset MSW_KEY_STORE
  check "B: gnome-keyring starts and unlocks a throwaway login keyring" \
    sh -c 'printf "%s" msw-check-keyring | gnome-keyring-daemon --unlock --components=secrets'
  check "B: backend selects secret-service" eq "$("$K" backend 2>&1)" secret-service
  export MSW_KEY_STORE=secret-service
  check "B: byte-exact round trip, delete, get after delete fails" roundtrip ss
  check "B: rotate, then mint-token.sh jwt verifies" rotate_then_jwt secret-service
  check "B: without the bus, get fails" \
    fails_saying "MSW_KEY_STORE=secret-service is not available" \
    env -u DBUS_SESSION_BUS_ADDRESS "$K" get "$SERVICE" "$CLIENT_ID"
  check "B: and nothing fell back to the file backend" no_key_dir
  local rc=0
  timeout 20 secret-tool lock --collection=/org/freedesktop/secrets/collection/login >/dev/null 2>&1 || rc=$?
  case "$rc" in
    0)
      check "B: with the collection locked, get fails within 20 seconds and says to unlock it" locked_get
      check "B: and nothing fell back to the file backend" no_key_dir ;;
    124) not_ok "B: secret-tool lock did not finish within 20 seconds" ;;
    *) skip "B: secret-tool lock is not available here (status $rc); the locked-collection check did not run" ;;
  esac
  phase_done B
}

# --- C: pass, as msw ----------------------------------------------------------

user_c() {
  unset MSW_KEY_STORE
  export GNUPGHOME="$HOME/.gnupg-check" PASSWORD_STORE_DIR="$HOME/.password-store-check"
  mkdir -p "$GNUPGHOME"
  chmod 700 "$GNUPGHOME"
  check "C: a throwaway gpg key without a passphrase" \
    gpg --batch --pinentry-mode loopback --passphrase '' \
        --quick-gen-key 'msw-check <msw-check@example.invalid>' default default never
  check "C: pass init" pass init msw-check@example.invalid
  check "C: backend selects pass" eq "$("$K" backend 2>&1)" pass
  export MSW_KEY_STORE=pass
  check "C: byte-exact round trip, delete, get after delete fails" roundtrip pa
  check "C: rotate, then mint-token.sh jwt verifies" rotate_then_jwt pass
  check "C: the store holds the entry encrypted" test -f "$PASSWORD_STORE_DIR/mintswarm/$SERVICE/$CLIENT_ID.gpg"
  gpgconf --kill all >/dev/null 2>&1 || true
  phase_done C
}

# --- D: the file backend, as msw ------------------------------------------------

file_dir() { printf '%s' "$HOME/.config/mintswarm/keys"; }
put_warns() {
  "$K" put msw-check fl < "$W/key1.pem" 2> "$HOME/file.err" || { cat "$HOME/file.err"; return 1; }
  grep -qF "warning: the file key store keeps the key unencrypted in $(file_dir)/msw-check@fl" "$HOME/file.err"
}
get_warns() {
  same_as "$W/key1.pem" fl 2> "$HOME/file.err" || { cat "$HOME/file.err"; return 1; }
  grep -qF "warning: the file key store keeps the key unencrypted" "$HOME/file.err"
}

user_d() {
  local d
  d="$(file_dir)"
  case "${1:-}" in
    put)
      unset MSW_KEY_STORE
      check "D: with no key store, backend fails and names MSW_KEY_STORE" \
        fails_saying "set MSW_KEY_STORE to one of" "$K" backend
      check "D: and the file backend was not picked" no_key_dir
      export MSW_KEY_STORE=file
      check "D: put stores the key and warns" put_warns
      check "D: the key file is 600" eq "$(stat -c %a "$d/msw-check@fl")" 600
      check "D: the key directory is 700" eq "$(stat -c %a "$d")" 700
      check "D: get returns it byte for byte and warns" get_warns
      chmod 640 "$d/msw-check@fl"
      check "D: get refuses a key file at 640" fails_saying "group or other permissions" "$K" get msw-check fl
      chmod 600 "$d/msw-check@fl"
      chmod 750 "$d"
      check "D: get refuses a key directory at 750" fails_saying "group or other permissions" "$K" get msw-check fl
      chmod 700 "$d"
      printf 'not a key\n' > "$HOME/not-a-key"
      chmod 600 "$HOME/not-a-key"
      ln -s "$HOME/not-a-key" "$d/msw-check@ln"
      check "D: put refuses a symlinked key file" fails_saying "symlink" "$K" put msw-check ln < "$W/key1.pem"
      check "D: nothing was written through the symlink" eq "$(cat "$HOME/not-a-key")" "not a key"
      rm -f "$d/msw-check@ln" "$HOME/not-a-key"
      check "D: put a key for the foreign-owner check" "$K" put msw-check fo < "$W/key2.pem"
      phase_done D-put ;;
    foreign)
      export MSW_KEY_STORE=file
      check "D: get refuses a key file owned by root" fails_saying "not owned by" "$K" get msw-check fo
      phase_done D-foreign ;;
    rest)
      export MSW_KEY_STORE=file
      check "D: delete removes the key files" sh -c '"$1" delete msw-check fo && "$1" delete msw-check fl && test ! -e "$2/msw-check@fl"' sh "$K" "$d"
      refuses "D: get after delete fails" "$K" get msw-check fl
      check "D: rotate, then mint-token.sh jwt verifies" rotate_then_jwt file
      phase_done D-rest ;;
    *) printf '%s\n' "check.sh: user-d needs put, foreign or rest" >&2; exit 2 ;;
  esac
}

# --- E: systemd-creds, as root ---------------------------------------------------

HOST_KEY=/var/lib/systemd/credential.secret
root_e() {
  export MSW_KEY_STORE=systemd-creds
  local cred="$HOME/.config/mintswarm/credstore/msw-check@sd.cred"
  check "E: systemd-creds setup creates the host key" sh -c 'systemd-creds setup && test -s "$1"' sh "$HOST_KEY"
  check "E: backend accepts systemd-creds" eq "$("$K" backend 2>&1)" systemd-creds
  check "E: byte-exact round trip, delete, get after delete fails" roundtrip rt
  check "E: put again" "$K" put msw-check sd < "$W/key1.pem"
  check "E: the .cred file holds no key line" sh -c '! grep -aqF -f "$1" "$2"' sh "$W/keylines" "$cred"
  mv "$HOST_KEY" "$W/credential.secret.aside"
  refuses "E: with the host key moved aside, get fails (the null key was not used)" "$K" get msw-check sd
  rm -f "$HOST_KEY"
  mv "$W/credential.secret.aside" "$HOST_KEY"
  check "E: with the host key back, get works again" same_as "$W/key1.pem" sd
  check "E: delete" "$K" delete msw-check sd
  check "E: rotate, then mint-token.sh jwt verifies" rotate_then_jwt systemd-creds
  phase_done E
}

# --- driver --------------------------------------------------------------------

msw=(setpriv --reuid=1000 --regid=1000 --init-groups
     env -i HOME="$MSW_HOME" TMPDIR="$MSW_HOME/tmp" PATH="$PATH_SAFE" USER=msw LOGNAME=msw
     LANG=C.UTF-8 XDG_RUNTIME_DIR="$MSW_RUN")
root_env=(env -i HOME=/root TMPDIR=/root/tmp PATH="$PATH_SAFE" USER=root LOGNAME=root LANG=C.UTF-8)
run_phase() {  # run_phase [-t] NAME SECONDS COMMAND...: run, show and count the phase's lines
  local traced=0 name secs rc=0
  if [ "$1" = -t ]; then traced=1; shift; fi
  name="$1"; secs="$2"; shift 2
  if [ "$traced" = 1 ]; then  # execve only, with full argv strings
    set -- strace -f -qq -e trace=execve -s 65535 -o "$W/strace.$name" "$@"
  fi
  printf -- '--- phase %s\n' "$name"
  timeout "$secs" "$@" > "$W/out.$name" 2>&1 || rc=$?
  grep -v '^phase-done ' "$W/out.$name" || true
  passed=$((passed + $(grep -c '^ok    ' "$W/out.$name" || true)))
  failed=$((failed + $(grep -c '^FAIL  ' "$W/out.$name" || true)))
  skipped=$((skipped + $(grep -c '^skip  ' "$W/out.$name" || true)))
  if [ "$rc" -eq 0 ] && grep -qx "phase-done $name" "$W/out.$name"; then
    ok "$name: ran to the end"
  else
    not_ok "$name: did not run to the end (status $rc)"
  fi
}

driver() {
  export HOME=/root
  mkdir -p "$W" /root/tmp "$MSW_HOME/tmp" "$MSW_RUN"
  chmod 755 "$W"
  chown msw:msw "$MSW_HOME/tmp" "$MSW_RUN"
  chmod 700 "$MSW_RUN"

  printf 'kernel: %s\n' "$(uname -srm)"
  printf 'packages: %s\n' "$(dpkg-query -W -f='${Package}=${Version} ' \
    systemd libsecret-tools gnome-keyring pass gnupg git openssl bash shellcheck strace)"

  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$W/key1.pem" 2>/dev/null
  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$W/key2.pem" 2>/dev/null
  openssl pkey -in "$W/key1.pem" -pubout -out "$W/pub1.pem"
  chmod 644 "$W"/*.pem
  {
    grep -hv -- '-----' "$W/key1.pem" "$W/key2.pem" | awk 'length($0) >= 32'
    openssl base64 -A -in "$W/key1.pem"; echo
    openssl base64 -A -in "$W/key2.pem"; echo
  } > "$W/keylines"

  # A ends with phase-done only when selftest.sh exits 0, that is when all its checks passed.
  run_phase A 120 "${msw[@]}" "$SELF" user-a
  run_phase -t B 120 "${msw[@]}" dbus-run-session -- "$SELF" user-b
  run_phase -t C 120 "${msw[@]}" "$SELF" user-c
  run_phase -t D-put 120 "${msw[@]}" "$SELF" user-d put
  chown root:root "$MSW_HOME/.config/mintswarm/keys/msw-check@fo" || true
  run_phase -t D-foreign 120 "${msw[@]}" "$SELF" user-d foreign
  chown msw:msw "$MSW_HOME/.config/mintswarm/keys/msw-check@fo" || true
  run_phase -t D-rest 120 "${msw[@]}" "$SELF" user-d rest
  run_phase -t E 120 "${root_env[@]}" "$SELF" root-e

  printf -- '--- phase F\n'
  local log
  for log in B C D-put D-foreign D-rest E; do
    check "F: no key line in any argv traced in $log" sh -c '! grep -qF -f "$1" "$2"' sh "$W/keylines" "$W/strace.$log"
    check "F: no PRIVATE KEY in any argv traced in $log" sh -c '! grep -qF "PRIVATE KEY" "$1"' sh "$W/strace.$log"
    check "F: the $log trace shows key-store.sh running (the trace is not vacuous)" \
      grep -qE 'execve\("[^"]*/key-store\.sh"' "$W/strace.$log"
  done
  check "F: the B trace shows secret-tool running" grep -qE 'execve\("[^"]*/secret-tool", \["secret-tool", "store"' "$W/strace.B"
  check "F: the C trace shows pass running" grep -qE 'execve\("[^"]*/pass", \["pass", "insert"' "$W/strace.C"
  check "F: the E trace shows systemd-creds encrypt, without --user as root" \
    grep -qE 'execve\("[^"]*/systemd-creds", \["systemd-creds", "encrypt"' "$W/strace.E"
  check "F: msw's TMPDIR is empty" eq "$(ls -A "$MSW_HOME/tmp")" ""
  check "F: root's TMPDIR is empty" eq "$(ls -A /root/tmp)" ""
  grep -rlaFs -D skip -f "$W/keylines" /home /root /tmp /run /dev/shm /var > "$W/plaintext" || true
  check "F: no plaintext key line on disk outside the file backend's key files" \
    sh -c '! grep -v "^$1/" "$2"' sh "$MSW_HOME/.config/mintswarm/keys" "$W/plaintext"
  check "F: the search is not vacuous: it finds the file backend's key" \
    grep -qx "$MSW_HOME/.config/mintswarm/keys/$SERVICE@$CLIENT_ID" "$W/plaintext"

  printf '\n%d passed, %d failed, %d skipped\n' "$passed" "$failed" "$skipped"
  [ "$failed" -eq 0 ]
}

refuse() { printf 'check.sh: %s\n' "$*" >&2; exit 2; }
MARKER=/etc/mintswarm-linux-check
{ [ -f /.dockerenv ] || [ -f /run/.containerenv ]; } \
  && [ "$(cat "$MARKER" 2>/dev/null || true)" = mintswarm-linux-check ] \
  || refuse "refusing to run outside the linux-check.sh container (no $MARKER; see linux-check.sh)"
case "${1:-}" in
  user-*)
    [ "$(id -un)" = msw ] && [ "${HOME:-}" = "$MSW_HOME" ] \
      || refuse "the ${1} steps run only as msw with HOME=$MSW_HOME" ;;
  *)
    [ "$(id -u)" = 0 ] || refuse "run as root inside the container" ;;
esac

case "${1:-}" in
  '') driver ;;
  user-a) user_a ;;
  user-b) user_b ;;
  user-c) user_c ;;
  user-d) user_d "${2:-}" ;;
  root-e) root_e ;;
  *) printf '%s\n' "usage: check.sh [user-a | user-b | user-c | user-d put|foreign|rest | root-e]" >&2; exit 2 ;;
esac
