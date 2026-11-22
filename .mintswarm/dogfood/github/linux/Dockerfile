# Image for linux-check.sh: the key store backends and the identity tooling on
# Debian. Built from this directory only; it holds no key and runs nothing on
# start. linux-check.sh streams the tracked files in at run time.
ARG BASE=debian:trixie-slim
FROM ${BASE}

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      bash jq openssl git python3 python3-yaml shellcheck \
      libsecret-tools gnome-keyring dbus dbus-bin dbus-daemon dbus-session-bus-common \
      pass gnupg systemd strace ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY yq /usr/local/bin/yq
# check.sh runs only where this marker is: never in another container, such as a
# toolbox or distrobox that shares the host's HOME and session bus.
RUN chmod 755 /usr/local/bin/yq \
 && useradd --create-home --uid 1000 --user-group --shell /bin/bash msw \
 && printf 'mintswarm-linux-check\n' > /etc/mintswarm-linux-check
