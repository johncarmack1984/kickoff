#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [ ! -f .vmod-build/libvmod_reqwest.so ]; then
    echo "--- Building vmod-reqwest (first run only) ---"
    bash build-vmod.sh
fi

echo "--- Starting services ---"
docker compose up --build -d

echo "--- Waiting for edge to accept connections ---"
for i in $(seq 1 30); do
    if curl -sf -o /dev/null http://localhost:8080/master.m3u8?token=kickoff-test 2>/dev/null; then
        echo "Edge is ready."
        echo ""
        echo "  Player URL:  http://localhost:8080/master.m3u8?token=kickoff-test"
        echo "  Smoke test:  bash smoke.sh"
        echo "  Logs:        docker compose logs -f"
        echo "  Stop:        docker compose down"
        exit 0
    fi
    sleep 1
done

echo "Edge did not become ready in 30s.  Check: docker compose logs"
exit 1
