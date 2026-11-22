#!/usr/bin/env bash
# Check the key store backends and the identity tooling on Linux, in a throwaway
# container. A recorded manual check, not a gate.
#
#   linux-check.sh
#
# Builds the image in linux/ (base $MSW_LINUX_BASE, default debian:trixie-slim;
# only the build needs the network, and its context is linux/ alone), prints the
# base image's digest, then streams the tracked files under .mintswarm/ of this
# worktree into a container and runs linux/check.sh there as root. The container
# runs with --network none, no volume, no variable passed in, no socket and no
# SSH agent: nothing of the host's Keychain, ~/.config/mintswarm, ~/.gnupg or
# ~/.password-store reaches it. Every key it uses is generated inside it.
#
# Exits with the container's status.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
image=mintswarm-dogfood-linux
base="${MSW_LINUX_BASE:-debian:trixie-slim}"

command -v docker >/dev/null 2>&1 || { printf '%s\n' "linux-check.sh: docker is not on PATH" >&2; exit 1; }

docker build -t "$image" --build-arg BASE="$base" "$here/linux"

digest="$(docker image inspect --format '{{join .RepoDigests " "}}' "$base" 2>/dev/null || true)"
printf 'base image: %s %s\n' "$base" "${digest:-(no digest recorded)}"
printf 'check image: %s\n' "$(docker image inspect --format '{{.Id}}' "$image")"
dirty=""
git -C "$root" diff --quiet HEAD -- .mintswarm || dirty=" (with uncommitted changes under .mintswarm)"
printf 'commit: %s%s\n' "$(git -C "$root" rev-parse HEAD)" "$dirty"

cd "$root"
rc=0
git ls-files -z .mintswarm | COPYFILE_DISABLE=1 tar --null -T - -cf - \
  | docker run --rm -i --network none "$image" \
      bash -c 'mkdir -p /src && tar --no-same-owner --warning=no-unknown-keyword -xf - -C /src && exec /src/.mintswarm/dogfood/github/linux/check.sh' \
  || rc=$?
printf 'linux-check.sh: exit status %d\n' "$rc"
exit "$rc"
