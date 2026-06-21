#!/bin/bash

# cafe-tui - Start the Terminal UI client

# Usage: ./start-tui.sh [OPTIONS]

# Options:
#   --url <URL>    cafe-server URL [default: http://localhost:4000]
#   --token <TOKEN> cafe-server API token
#   -h, --help     Show this help message

# Example:
#   ./start-tui.sh
#   ./start-tui.sh --url http://localhost:4000 --token my-token

# Set default values from environment variables
URL=${CAFE_SERVER_URL:-http://localhost:4000}
TOKEN=${CAFE_TOKEN:-}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --url=*) URL=${1#*=};;
    --token=*) TOKEN=${1#*=};;
    -h|--help) echo "Usage: $0 [OPTIONS]"; echo; echo "Options:"; echo "  --url <URL>    cafe-server URL [default: http://localhost:4000]"; echo "  --token <TOKEN> cafe-server API token"; echo "  -h, --help     Show this help message"; exit 0;;
    *) echo "Error: Unknown option $1"; echo; echo "Usage: $0 [OPTIONS]"; exit 1;;
  esac
  shift
done

# Check if cafe-tui is available
if [[ ! -f ./target/debug/cafe-tui ]]; then
  echo "Error: cafe-tui not found in ./target/debug/" >&2
  echo "Please run: cd cafe-tui && cargo build" >&2
  exit 1
fi

# Build if needed (optional - the binary should already exist)
# cargo build --release &>/dev/null && echo "Built cafe-tui" &>/dev/null

# Run the app
exec ./target/debug/cafe-tui --url "$URL" --token "$TOKEN"
