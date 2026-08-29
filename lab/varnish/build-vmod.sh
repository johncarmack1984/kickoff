#!/bin/bash
set -euo pipefail

VMOD_SRC="${VMOD_REQWEST_SRC:-$HOME/coding/varnish/vmod-reqwest}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="$SCRIPT_DIR/.vmod-build"

if [ ! -d "$VMOD_SRC" ]; then
    echo "error: vmod-reqwest source not found at $VMOD_SRC"
    echo "set VMOD_REQWEST_SRC to the checkout containing copy_headers_to_resp()"
    exit 1
fi

echo "=== Building vmod-reqwest from $VMOD_SRC ==="

docker build -t vmod-reqwest-builder -f - "$SCRIPT_DIR" <<'DOCKERFILE'
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
WORKDIR /app
DOCKERFILE

mkdir -p "$OUT_DIR"

docker run --rm \
    -v "$VMOD_SRC:/app-src:ro" \
    -v "$OUT_DIR:/out" \
    -e CARGO_TARGET_DIR=/tmp/target \
    vmod-reqwest-builder \
    sh -c 'cp -a /app-src /build && cd /build && cargo build 2>&1 && cp /tmp/target/debug/libvmod_reqwest.so /out/ && echo "=== vmod built ===" && ls -lh /out/libvmod_reqwest.so'
