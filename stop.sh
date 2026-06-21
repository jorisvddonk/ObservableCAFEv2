#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "Stopping ObservableCAFE..."
process-compose down --port 8082
