#!/usr/bin/env bash
# Install the web/ workspace's dependencies from its lockfile. Run by the typescript-* gates.
#
#   web-install.sh
#
# Changes to the top of the git work tree and runs
# pnpm --dir web install --frozen-lockfile --ignore-scripts --ignore-pnpmfile,
# so the install runs no repository code: no .pnpmfile.cjs, no lifecycle
# script of any package.json and no dependency build script. The readiness
# check keeps `pnpm install` unreadable, so the gates install through this
# tracked script, which the floors protect. It writes only node_modules
# under web/ (the workspace root's and each workspace package's, all
# git-ignored) and pnpm's own store, never next to sources.
#
# The readiness check's gate-tools signal checks that pnpm is on PATH for
# the gates that run this script.
#
# Exits with pnpm's status, and 2 when git fails or pnpm is not on PATH.
# Needs git and pnpm.
set -euo pipefail

root="$(git rev-parse --show-toplevel)" || exit 2
cd "$root" || exit 2

if ! command -v pnpm >/dev/null 2>&1; then
    printf '%s\n' "web-install.sh: pnpm is not on PATH" >&2
    exit 2
fi

exec pnpm --dir web install --frozen-lockfile --ignore-scripts --ignore-pnpmfile
