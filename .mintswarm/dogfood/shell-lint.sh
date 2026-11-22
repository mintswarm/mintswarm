#!/usr/bin/env bash
# Lint every tracked shell script at warning level. Run by the shell-lint gate.
#
#   shell-lint.sh
#
# Changes to the top of the git work tree, lists the tracked *.sh files with
# git ls-files and runs shellcheck --norc -S warning -x -P SCRIPTDIR over them.
# -P SCRIPTDIR makes a "# shellcheck source=" directive resolve against the
# script's own directory wherever this is run from; --norc keeps any
# .shellcheckrc from changing what the gate checks, so only directives in the
# linted scripts themselves can. New scripts are covered once they are tracked.
#
# Exits with shellcheck's status, 0 when no script is tracked, and 2 when git
# fails or shellcheck is not on PATH. Needs git and shellcheck.
set -euo pipefail

root="$(git rev-parse --show-toplevel)" || exit 2
cd "$root" || exit 2

if ! command -v shellcheck >/dev/null 2>&1; then
    printf '%s\n' "shell-lint.sh: shellcheck is not on PATH" >&2
    exit 2
fi

list="$(mktemp "${TMPDIR:-/tmp}/msw-shell-lint.XXXXXX")" || exit 2
trap 'rm -f "$list"' EXIT
git ls-files -z -- '*.sh' >"$list" || exit 2

files=()
while IFS= read -r -d '' file; do
    files+=("$file")
done <"$list"

if [ "${#files[@]}" -eq 0 ]; then
    exit 0
fi

status=0
shellcheck --norc -S warning -x -P SCRIPTDIR -- "${files[@]}" || status=$?
exit "$status"
