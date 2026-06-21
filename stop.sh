#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "Stopping ObservableCAFE..."
process-compose kill --port 8082 2>/dev/null || true
process-compose down --port 8082 2>/dev/null || true
sleep 1

# Fallback: kill anything still on the process-compose port
if lsof -ti:8082 >/dev/null 2>&1; then
    echo "Force-killing lingering processes on port 8082..."
    lsof -ti:8082 | xargs kill -9 2>/dev/null || true
    sleep 1
fi

echo "Stopped."
