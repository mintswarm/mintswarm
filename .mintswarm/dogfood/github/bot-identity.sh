#!/usr/bin/env bash
# Make every commit on a factory branch (mintswarm/*) the bee's, with the login and e-mail
# derived from its config ($MSW_APP_CONFIG; common.sh, msw_bot_login, msw_bot_email).
#
#   bot-identity.sh install     write ~/.config/mintswarm/bot-identity.gitconfig and include
#                               it from this repository's shared config on mintswarm/* branches
#   bot-identity.sh status      show the identity git uses here and whether the include is set
#   bot-identity.sh uninstall   remove the include (the gitconfig file stays)
#
# install refuses, writing nothing, when the config has no usable slug or bot_user_id.
# After an App rename, edit the slug in the config and run install again.
#
# Run from any worktree of the repository: "git config --local" writes the
# repository's shared config, so the include applies in every worktree and to
# commits rewritten by a rebase. It does not apply on a detached HEAD; push.sh
# refuses any commit that is not the bot's. Running install again is safe.
#
# The included file lives outside the repository on purpose: including a tracked
# file would let whatever branch is checked out set arbitrary git config.
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

IDENTITY_FILE="$MSW_CONFIG_DIR/bot-identity.gitconfig"
INCLUDE_KEY="includeIf.onbranch:${MSW_BRANCH_PREFIX}.path"

usage() { printf '%s\n' "usage: bot-identity.sh install | status | uninstall" >&2; exit 2; }

git rev-parse --git-dir >/dev/null 2>&1 || msw_die "not inside a git repository"

case "${1:-}:$#" in
  install:1)
    login=""
    email=""
    login="$(msw_bot_login)" || exit 1
    email="$(msw_bot_email)" || exit 1
    mkdir -p "$MSW_CONFIG_DIR"
    git config --file "$IDENTITY_FILE" user.name "$login"
    git config --file "$IDENTITY_FILE" user.email "$email"
    git config --local --replace-all "$INCLUDE_KEY" "$IDENTITY_FILE"
    printf '%s\n' "installed: commits on ${MSW_BRANCH_PREFIX}* are authored and committed by $login"
    printf '%s\n' "include: $INCLUDE_KEY = $IDENTITY_FILE"
    ;;
  status:1)
    branch="$(git symbolic-ref --quiet --short HEAD || printf '%s' '(detached HEAD)')"
    include="$(git config --local --get-all "$INCLUDE_KEY" || true)"
    printf 'branch:    %s\n' "$branch"
    printf 'author:    %s\n' "$(git var GIT_AUTHOR_IDENT)"
    printf 'committer: %s\n' "$(git var GIT_COMMITTER_IDENT)"
    if [ -n "$include" ]; then
      printf 'include:   %s\n' "$include"
    else
      printf 'include:   not installed\n'
    fi
    ;;
  uninstall:1)
    git config --local --unset-all "$INCLUDE_KEY" || true
    git config --local --remove-section "includeIf.onbranch:${MSW_BRANCH_PREFIX}" 2>/dev/null || true
    printf '%s\n' "uninstalled: $INCLUDE_KEY removed"
    ;;
  *)
    usage ;;
esac
