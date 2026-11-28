#!/usr/bin/env bash
# Push a factory branch to GitHub as mintswarm-dev[bot].
#
#   push.sh BRANCH [--force-with-lease=EXPECTED_SHA]
#
# Pushes refs/heads/BRANCH to the same name at https://github.com/$MSW_REPO.git (MSW_REPO is
# owner/name, mintswarm/mintswarm by default; see common.sh),
# never through a named remote: origin may be an SSH remote, which would
# authenticate with the maintainer's key and record the push as theirs.
#
# WORK is the work branch, the branch the factory integrates into, read from
# ../project-setup.yaml through msw_work_branch (common.sh); push.sh exits 1 when
# it cannot be read.
#
# Refuses, before anything reaches the network:
#   - a branch outside mintswarm/, the work branch, or a branch that does not exist locally;
#   - a missing refs/remotes/origin/WORK (fetch first) or an empty origin/WORK..BRANCH;
#   - any commit in origin/WORK..BRANCH whose author or committer is not exactly
#     mintswarm-dev[bot] <359856778+mintswarm-dev[bot]@users.noreply.github.com>;
#   - repository config matching url.*, credential.*, http.* or remote.<URL>.*
#     (a remote section named after a URL applies to a push to that URL, so its
#     url or pushurl would redirect it), which could change where the push goes
#     or how it authenticates;
#   - --force, a bare --force-with-lease, or any other option.
# With --force-with-lease=SHA the push succeeds only if the branch on GitHub is at SHA
# (a URL push has no remote-tracking ref for a bare lease to use).
#
# The token is $MSW_TOKEN, or minted when unset: with contents:write, plus
# workflows:write when anything under .github/workflows/ is added, edited or deleted
# either in origin/WORK...BRANCH (from the merge base) or, with a lease, between the
# lease commit and BRANCH, as GitHub refuses a whole push from an App that touches a
# workflow file without that permission. The second diff catches a rebase onto a
# work branch that gained a workflow change; with a lease, the lease commit must be
# present locally. A branch that differs from the work branch under .github/workflows/
# asks for it on every push, for as long as it differs, even when the new commits touch
# no workflow file. It reaches git
# only through the environment and an inline credential helper (common.sh,
# msw_git_net); it is never in the URL, .git/config, a credential store or argv.
# git runs with https as the only transport (file too for a file:// MSW_PUSH_URL), so
# nothing can turn the push into ssh, and with hooks disabled (core.hooksPath=/dev/null
# and --no-verify), so no hook sees MSW_TOKEN in its environment.
# MSW_PUSH_URL overrides the URL only when it starts with file:// (selftest.sh).
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() { printf '%s\n' "usage: push.sh BRANCH [--force-with-lease=EXPECTED_SHA]" >&2; exit 2; }

[ $# -ge 1 ] && [ $# -le 2 ] || usage
branch="$1"
lease=""
if [ $# -eq 2 ]; then
  case "$2" in
    --force-with-lease=*) lease="${2#--force-with-lease=}" ;;
    --force-with-lease|--force|-f) msw_die "$2 is refused: use --force-with-lease=EXPECTED_SHA" ;;
    *) usage ;;
  esac
  case "$lease" in
    *[!0-9a-f]*|'') msw_die "--force-with-lease needs the full commit id the branch is expected at on GitHub" ;;
  esac
  [ ${#lease} -eq 40 ] || [ ${#lease} -eq 64 ] \
    || msw_die "--force-with-lease needs the full commit id the branch is expected at on GitHub"
fi

work="$(msw_work_branch)" || exit 1

case "$branch" in
  -*) usage ;;
  "$work") msw_die "refusing to push $work" ;;
  "$MSW_BRANCH_PREFIX"?*) ;;
  *) msw_die "refusing $branch: only ${MSW_BRANCH_PREFIX}* branches are pushed as the bot" ;;
esac
git check-ref-format "refs/heads/$branch" || msw_die "not a valid branch name: $branch"
git rev-parse --verify --quiet "refs/heads/$branch^{commit}" >/dev/null \
  || msw_die "no local branch $branch"
git rev-parse --verify --quiet "refs/remotes/origin/$work^{commit}" >/dev/null \
  || msw_die "no refs/remotes/origin/$work: fetch first"

count="$(git rev-list --count "refs/remotes/origin/$work..refs/heads/$branch")"
[ "$count" -gt 0 ] || msw_die "nothing to push: origin/$work..$branch is empty"

expected="$MSW_BOT_NAME|$MSW_BOT_EMAIL|$MSW_BOT_NAME|$MSW_BOT_EMAIL"
log="$(git log --no-color --no-show-signature --no-mailmap \
         --format='%H %an|%ae|%cn|%ce' "refs/remotes/origin/$work..refs/heads/$branch")" \
  || msw_die "cannot list origin/$work..$branch"
[ "$(printf '%s\n' "$log" | wc -l | tr -d ' ')" -eq "$count" ] \
  || msw_die "cannot list origin/$work..$branch"
bad="$(printf '%s\n' "$log" | while IFS= read -r line; do
  [ "${line#* }" = "$expected" ] || printf 'not the bot: %s\n' "$line"
done)"
if [ -n "$bad" ]; then
  printf '%s\n' "$bad" >&2
  msw_die "refusing: every commit in origin/$work..$branch must be authored and committed by $MSW_BOT_NAME (see README.md, Pitfalls)"
fi

risky="$(msw_git_env config --name-only --get-regexp '^(url\.|credential\.|http\.|remote\..*://)' || true)"
[ -z "$risky" ] || msw_die "refusing: repository config sets $(printf '%s' "$risky" | tr '\n' ' ')- remove it first"

url="$MSW_REPO_URL"
if [ -n "${MSW_PUSH_URL:-}" ]; then
  case "$MSW_PUSH_URL" in
    file://*) url="$MSW_PUSH_URL"; MSW_GIT_ALLOW_FILE=1 ;;
    *) msw_die "MSW_PUSH_URL may only be a file:// URL" ;;
  esac
fi

# From the top level, so the pathspec does not depend on the current directory.
top="$(git rev-parse --show-toplevel)" && [ -n "$top" ] || msw_die "cannot find the top of the work tree"
workflows_touched() {  # workflows_touched REV...: 0 when git diff REV... touches .github/workflows/
  local rc=0
  git -C "$top" diff --quiet --no-ext-diff --no-textconv "$@" -- .github/workflows || rc=$?
  case "$rc" in
    0) return 1 ;;
    1) return 0 ;;
    *) msw_die "cannot diff $*" ;;
  esac
}
perms='{"contents":"write"}'
if workflows_touched "refs/remotes/origin/$work...refs/heads/$branch"; then
  perms='{"contents":"write","workflows":"write"}'
fi
if [ -n "$lease" ]; then
  git cat-file -e "$lease^{commit}" 2>/dev/null \
    || msw_die "the lease commit $lease is not here: fetch the branch's old tip first"
  if workflows_touched "$lease" "refs/heads/$branch"; then
    perms='{"contents":"write","workflows":"write"}'
  fi
fi

tok=""
tok="$(msw_token "$perms")" || exit 1
MSW_TOKEN="$tok"
export MSW_TOKEN
tok=""

if [ -n "$lease" ]; then
  msw_git_net push --no-verify "--force-with-lease=refs/heads/$branch:$lease" \
    "$url" "refs/heads/$branch:refs/heads/$branch"
else
  msw_git_net push --no-verify "$url" "refs/heads/$branch:refs/heads/$branch"
fi
