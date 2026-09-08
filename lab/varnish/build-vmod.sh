#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PIN_DIR="$SCRIPT_DIR/vmod"
OUT_DIR="$SCRIPT_DIR/.vmod-build"

REV="$(sed -n 's/.*rev = "\([0-9a-f]*\)".*/\1/p' "$PIN_DIR/Cargo.toml")"
echo "=== Building vmod-reqwest @ ${REV:0:12} (pinned in vmod/Cargo.toml) ==="

# Dockerfile from stdin, no build context: nothing from the host is baked in.
docker build -t vmod-reqwest-builder - <<'DOCKERFILE'
FROM varnish:latest
USER root
RUN set -e; \
    apt-get update; \
    apt-get install -y curl ca-certificates build-essential clang pkg-config libssl-dev; \
    curl -Ls https://packages.varnish-software.com/varnish/bootstrap-deb.sh | sh; \
    apt-get update; \
    apt-get install -y varnish-dev; \
    rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:$PATH"
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
WORKDIR /build
DOCKERFILE

mkdir -p "$OUT_DIR"

# vmod/ is a pin-only Cargo package: its git dependency on vmod_reqwest carries
# the rev, Cargo.lock carries the rest of the tree, and `cargo build -p` builds
# the pinned checkout as the cdylib it declares. --locked refuses to drift.
docker run --rm \
    -v "$PIN_DIR:/pin:ro" \
    -v "$OUT_DIR:/out" \
    vmod-reqwest-builder \
    sh -c 'cp /pin/Cargo.toml /pin/Cargo.lock /build/ && cp -r /pin/src /build/src \
        && cargo build --release --locked -p vmod_reqwest \
        && cp target/release/libvmod_reqwest.so /out/ \
        && echo "=== vmod built ===" && ls -lh /out/libvmod_reqwest.so'
