#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "Restarting ObservableCAFE..."
process-compose kill --port 8082 2>/dev/null || true
process-compose down --port 8082 2>/dev/null || true
sleep 1
cargo build --workspace
process-compose up -D --port 8082
