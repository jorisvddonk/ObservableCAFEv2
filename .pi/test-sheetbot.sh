#!/usr/bin/env bash
set -e

SOCK=/tmp/cafe-bus.sock
test -S "$SOCK" || { echo "bus not found at $SOCK"; exit 1; }

# 1. List existing sessions
echo "=== Existing sessions ==="
echo '{"op":"list_sessions"}' | nc -U "$SOCK" 2>/dev/null | head -1 | python3 -m json.tool || true

# 2. Create a test session
SID="test-sheetbot-$(uuidgen | cut -d- -f1)"
echo "=== Creating session $SID ==="
MSG=$(cat <<END
{"op":"create_session","session_id":"$SID","agent_id":"sheetbot"}
END
)
echo "$MSG" | nc -U "$SOCK" 2>/dev/null | head -1 | python3 -m json.tool || true
sleep 0.5

# 3. Subscribe then publish a sheetbot.list_tasks RPC request
CALL_ID="call-$(uuidgen | cut -d- -f1)"
RPC_REQ=$(cat <<END
{
  "jsonrpc": "2.0",
  "id": "$CALL_ID",
  "method": "sheetbot.list_tasks",
  "params": {}
}
END
)
PUBLISH_MSG=$(cat <<END
{"op":"publish","session_id":"$SID","chunk":{"id":"$(uuidgen)","content_type":"null","content":null,"data":null,"mime_type":null,"producer":"test-script","annotations":{"jsonrpc.request":$RPC_REQ},"timestamp":$(date +%s)}}}
END
)

echo "=== Publishing sheetbot.list_tasks RPC ==="
echo "$PUBLISH_MSG" | nc -U "$SOCK" 2>/dev/null &
sleep 1

# Subscribe and watch for responses
echo "=== Watching for response (timeout 5s) ==="
echo "{\"op\":\"subscribe\",\"session_id\":\"$SID\"}" | timeout 5 nc -U "$SOCK" 2>/dev/null | while read line; do
  echo "$line" | python3 -c "
import sys, json
try:
    msg = json.loads(sys.stdin.readline())
    if msg.get('event') == 'chunk':
        c = msg.get('chunk', {})
        if 'jsonrpc.response' in c.get('annotations', {}):
            print('RPC RESPONSE:', json.dumps(c['annotations']['jsonrpc.response'], indent=2))
        elif c.get('content_type') == 'text':
            print('TEXT CHUNK:', c.get('content', '')[:500])
    elif msg.get('event') == 'history_complete':
        pass  # ignore replay
    else:
        print('EVENT:', json.dumps(msg, indent=2))
except Exception as e:
    print('PARSE ERROR:', e, '| line:', line[:200])
" 2>/dev/null || true
done

echo "=== Cleaning up session $SID ==="
echo "{\"op\":\"delete_session\",\"session_id\":\"$SID\"}" | nc -U "$SOCK" 2>/dev/null || true
