#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "Building Rust crates..."
cargo build --workspace

echo "Starting CAFE..."
process-compose up -D --port 8082
