#!/bin/bash

# cafe-tui - Start the Terminal UI client

# Usage: ./start-tui.sh [OPTIONS]

# Options:
#   --url <URL>              cafe-server URL [default: http://localhost:4000]
#   --token <TOKEN>          cafe-server API token
#   --new                    Create a new session on startup
#   --model <MODEL>          Preset model for the new session
#   --system-prompt <PROMPT> Preset system prompt for the new session
#   -h, --help               Show this help message

# Example:
#   ./start-tui.sh
#   ./start-tui.sh --url http://localhost:4000 --token my-token
#   ./start-tui.sh --new --model gemma3:1b --system-prompt "You are helpful."

# Set default values from environment variables
URL=${CAFE_SERVER_URL:-http://localhost:4000}
TOKEN=${CAFE_TOKEN:-}
NEW=""
MODEL=""
SYSTEM_PROMPT=""

# Parse command line arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --url=*) URL=${1#*=};;
    --url) URL="$2"; shift;;
    --token=*) TOKEN=${1#*=};;
    --token) TOKEN="$2"; shift;;
    --new) NEW="--new";;
    --model=*) MODEL=${1#*=};;
    --model) MODEL="$2"; shift;;
    --system-prompt=*) SYSTEM_PROMPT=${1#*=};;
    --system-prompt) SYSTEM_PROMPT="$2"; shift;;
    -h|--help) echo "Usage: $0 [OPTIONS]"; echo; echo "Options:"; echo "  --url <URL>              cafe-server URL [default: http://localhost:4000]"; echo "  --token <TOKEN>          cafe-server API token"; echo "  --new                    Create a new session on startup"; echo "  --model <MODEL>          Preset model for the new session"; echo "  --system-prompt <PROMPT> Preset system prompt for the new session"; echo "  -h, --help               Show this help message"; exit 0;;
    *) echo "Error: Unknown option $1"; echo; echo "Usage: $0 [OPTIONS]"; exit 1;;
  esac
  shift
done

# Build cafe-tui if the binary is missing or outdated
if [[ ! -f ./target/debug/cafe-tui ]] || [[ ./cafe-tui/src/main.rs -nt ./target/debug/cafe-tui ]]; then
  echo "Building cafe-tui..."
  cargo build -p cafe-tui
fi

# Build if needed (optional - the binary should already exist)
# cargo build --release &>/dev/null && echo "Built cafe-tui" &>/dev/null

# Run the app
exec ./target/debug/cafe-tui --url "$URL" --token "$TOKEN" $NEW ${MODEL:+--model "$MODEL"} ${SYSTEM_PROMPT:+--system-prompt "$SYSTEM_PROMPT"}
