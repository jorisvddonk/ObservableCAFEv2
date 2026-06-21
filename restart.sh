#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "Restarting ObservableCAFE..."
process-compose down --port 8082
cargo build --workspace
process-compose up -D --port 8082
