# syntax=docker/dockerfile:1
# The webhook receiver, `mintswarm-server webhooks`, built from the workspace.
# Build from the repository root: docker build -f docker/webhooks/Dockerfile .
# The Rust toolchain comes from rust-toolchain.toml alone, through rustup.

ARG BASE=debian:trixie-slim

FROM ${BASE} AS builder
RUN apt-get update \
 && apt-get install -y --no-install-recommends rustup gcc libc6-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY rust-toolchain.toml ./
# Any cargo call installs the toolchain rust-toolchain.toml names, in its own layer.
RUN cargo --version
COPY . .
# Cargo writes to build/target (.cargo/config.toml), kept on a cache mount, so
# no build output reaches a layer; the binary is copied out in the same step.
RUN --mount=type=cache,target=/src/build \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    cargo build --release --locked -p mintswarm-server \
 && cp build/target/release/mintswarm-server /usr/local/bin/mintswarm-server

FROM ${BASE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --user-group --no-create-home --shell /usr/sbin/nologin msw \
 && mkdir -p /var/log/mintswarm \
 && chown msw:msw /var/log/mintswarm
COPY --from=builder /usr/local/bin/mintswarm-server /usr/local/bin/mintswarm-server
USER msw
ENTRYPOINT ["mintswarm-server", "webhooks"]
