#!/usr/bin/env python3
"""Test the cafe-sheetbot bridge by sending an RPC request and reading the response."""
import json, socket, uuid, threading, time, sys

SOCKET = "/tmp/cafe-bus.sock"

def send(sock, msg):
    sock.sendall((json.dumps(msg) + "\n").encode())

def recv(sock, timeout=3):
    sock.settimeout(timeout)
    f = sock.makefile("r")
    line = f.readline()
    return json.loads(line) if line else None

def main():
    method = sys.argv[1] if len(sys.argv) > 1 else "list_tasks"
    params = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(SOCKET)

    # List sessions to find an existing one
    send(sock, {"op": "list_sessions"})
    resp = recv(sock)
    sessions = resp.get("sessions", [])
    print(f"Found {len(sessions)} sessions:")
    for s in sessions:
        print(f"  {s['session_id']} (agent={s.get('agent_id','?')})")

    # Pick the first session that exists, or create one
    if sessions:
        sid = sessions[0]["session_id"]
        print(f"\nUsing existing session: {sid}")
    else:
        sid = f"test-{uuid.uuid4().hex[:8]}"
        send(sock, {"op": "create_session", "session_id": sid, "agent_id": "sheetbot"})
        resp = recv(sock)
        print(f"\nCreated session: {sid}")
        time.sleep(2.5)  # wait for bridge to discover it

    # Subscribe to the session
    send(sock, {"op": "subscribe", "session_id": sid})
    # Drain history replay
    while True:
        msg = recv(sock, timeout=2)
        if msg is None:
            break
        if msg.get("event") == "history_complete":
            break

    time.sleep(0.5)  # let bridge subscribe too

    # Publish the RPC request
    call_id = f"call-{uuid.uuid4().hex[:8]}"
    rpc_req = {
        "jsonrpc": "2.0", "id": call_id,
        "method": f"sheetbot.{method}", "params": params,
    }
    chunk = {
        "id": uuid.uuid4().hex, "content_type": "null",
        "content": None, "data": None, "mime_type": None,
        "producer": "test-script",
        "annotations": {"jsonrpc.request": rpc_req},
        "timestamp": int(time.time() * 1000),
    }
    send(sock, {"op": "publish", "session_id": sid, "chunk": chunk})
    print(f"\n>>> Published sheetbot.{method} (call_id={call_id})")
    print("--- Waiting for response ---")

    # Read responses
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            msg = recv(sock, timeout=1)
        except socket.timeout:
            continue
        if msg is None:
            break
        event = msg.get("event")
        if event == "chunk":
            chunk = msg.get("chunk", {})
            ann = chunk.get("annotations", {})
            if "jsonrpc.response" in ann:
                print("\n=== RPC RESPONSE ===")
                print(json.dumps(ann["jsonrpc.response"], indent=2))
                return
            elif chunk.get("content_type") == "text":
                content = chunk.get("content", "")
                print(f"\n=== RESULT TEXT ({len(content)} chars) ===")
                try:
                    print(json.dumps(json.loads(content), indent=2)[:2000])
                except json.JSONDecodeError:
                    print(content[:500])
                return
        elif event != "history_complete":
            print(f"EVENT: {json.dumps(msg, indent=2)[:200]}")

    print("\n*** No response received within timeout ***")

if __name__ == "__main__":
    main()
