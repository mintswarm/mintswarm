#!/usr/bin/env bash
# Run gh as the bee, as its config names it, against $MSW_REPO (owner/name, mintswarm/mintswarm by default).
#
#   bot-gh.sh ARGS...      for example: bot-gh.sh api repos/{owner}/{repo} --jq .full_name
#
# The token is $MSW_TOKEN, or a freshly minted installation token when unset; gh
# is not run at all when minting fails, so it never falls back to the maintainer's
# stored login. The token reaches gh only as GH_TOKEN in the environment.
# GITHUB_TOKEN, GH_ENTERPRISE_TOKEN and GITHUB_ENTERPRISE_TOKEN are removed;
# GH_HOST=github.com and GH_REPO=$MSW_REPO are set, so gh api fills
# {owner} and {repo} with that repository. Prompts are disabled.
#
# Open PRs with "bot-gh.sh api repos/{owner}/{repo}/pulls ...", never with
# "gh pr create", which may push the branch through origin itself.
set -euo pipefail

# shellcheck source=common.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

tok=""
tok="$(msw_token)" || exit 1

unset GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN MSW_TOKEN
GH_TOKEN="$tok"
GH_HOST=github.com
GH_REPO="$MSW_REPO_OWNER/$MSW_REPO_NAME"
GH_PROMPT_DISABLED=1
export GH_TOKEN GH_HOST GH_REPO GH_PROMPT_DISABLED
tok=""

exec gh "$@"
