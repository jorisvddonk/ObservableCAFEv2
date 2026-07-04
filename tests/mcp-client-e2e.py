#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["toml"]
# ///
"""
End-to-end test: cafe-mcp-client MCP bridge on the bus.

Starts a fake MCP server (Python, stdio), configures cafe-mcp-client
to connect to it, creates a session, publishes a tool.call with
provider:mcp, and verifies the tool.result comes back.

Usage:
    cargo build --release
    uv run tests/mcp-client-e2e.py
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
MCP_CLIENT_BIN = os.path.join(RELEASE_DIR, "cafe-mcp-client")


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def recv_line(sock):
    data = b""
    while True:
        b = sock.recv(1)
        if not b or b == b"\n":
            break
        data += b
    return data


def send_msg(sock, msg):
    sock.sendall((json.dumps(msg) + "\n").encode())


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI), ("cafe-mcp-client", MCP_CLIENT_BIN)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        mcp_servers_cfg = os.path.join(tmpdir, "mcp-servers.toml")
        fake_mcp_script = os.path.join(tmpdir, "fake_mcp.py")

        # Write the fake MCP server
        with open(fake_mcp_script, "w") as f:
            f.write("""#!/usr/bin/env python3
import json, sys

def main():
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        msg = json.loads(line)
        method = msg.get("method")
        msg_id = msg.get("id")

        if method == "initialize":
            sys.stdout.write(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {"name": "fake-mcp", "version": "1.0"},
                    "capabilities": {"tools": {}}
                }
            }) + "\\n")
            sys.stdout.flush()
        elif method == "tools/list":
            sys.stdout.write(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {
                    "tools": [
                        {
                            "name": "test_greet",
                            "description": "Greet someone",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "name": {"type": "string"}
                                },
                                "required": ["name"]
                            }
                        }
                    ]
                }
            }) + "\\n")
            sys.stdout.flush()
        elif method == "tools/call":
            args = msg.get("params", {}).get("arguments", {})
            name = args.get("name", "world")
            sys.stdout.write(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {
                    "content": [
                        {"type": "text", "text": json.dumps({"greeting": f"Hello, {name}!"})}
                    ]
                }
            }) + "\\n")
            sys.stdout.flush()
        elif method == "notifications/initialized":
            pass
        else:
            sys.stdout.write(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "error": {"code": -32601, "message": f"unknown: {method}"}
            }) + "\\n")
            sys.stdout.flush()

if __name__ == "__main__":
    main()
""")
        os.chmod(fake_mcp_script, 0o755)

        # Write mcp-servers.toml
        with open(mcp_servers_cfg, "w") as f:
            f.write(f"""[[server]]
name = "fake-test"
command = "{sys.executable}"
args = ["{fake_mcp_script}"]
""")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["CAFE_MCP_SERVERS"] = mcp_servers_cfg

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-mcp-client ===", file=sys.stderr)
        client_proc = subprocess.Popen(
            [MCP_CLIENT_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        time.sleep(3)

        # Create a session via cafe-cli
        print("=== Create session ===", file=sys.stderr)
        r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
        assert r.returncode == 0, f"create-session failed: {r.stderr}"
        session_id = r.stdout.strip()
        print(f"  session={session_id}", file=sys.stderr)
        time.sleep(1)

        # Subscribe to the session (drain history)
        sub_sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sub_sock.connect(bus_socket)
        line = recv_line(sub_sock)  # Connected
        send_msg(sub_sock, {"op": "subscribe", "session_id": session_id})
        while True:
            msg = json.loads(recv_line(sub_sock))
            if msg.get("event") == "history_complete":
                break

        # Publish a tool.call chunk with provider:mcp
        call_id = str(uuid.uuid4())
        now_ms = int(time.time() * 1000)
        send_msg(sub_sock, {
            "op": "publish",
            "session_id": session_id,
            "chunk": {
                "id": str(uuid.uuid4()),
                "content_type": "null",
                "content": None,
                "data": None,
                "mime_type": None,
                "producer": "com.nominal.mcp-test",
                "timestamp": now_ms,
                "annotations": {
                    "cafe.tool.call": {
                        "name": "test_greet",
                        "parameters": {"name": "Sweden"},
                        "provider": "mcp"
                    },
                    "cafe.transient": True,
                    "cafe.transient.retain_secs": 60,
                },
            }
        })
        print(f"  published tool.call: test_greet (provider=mcp)", file=sys.stderr)

        # Read chunks looking for tool.result with provider=mcp
        print("=== Reading tool.result ===", file=sys.stderr)
        sub_sock.settimeout(15)
        found = False
        try:
            while True:
                line = recv_line(sub_sock)
                if not line:
                    break
                msg = json.loads(line)
                if msg.get("event") == "chunk":
                    ann = msg["chunk"].get("annotations", {})
                    tool_result = ann.get("cafe.tool.result")
                    if tool_result and tool_result.get("provider") == "mcp":
                        output = tool_result.get("output", {})
                        print(f"  tool.result: {json.dumps(output, indent=2)}", file=sys.stderr)
                        assert tool_result["name"] == "test_greet"
                        assert output.get("greeting") == "Hello, Sweden!"
                        found = True
                        break
        except socket.timeout:
            print("  timeout waiting for tool.result", file=sys.stderr)

        assert found, "No MCP tool.result received"

        # Cleanup
        sub_sock.close()
        run([CLI, "--bus", bus_socket, "delete-session", session_id])

        client_proc.kill()
        client_proc.wait()
        bus_proc.kill()
        bus_proc.wait()

    print(file=sys.stderr)
    print("=== ALL MCP CLIENT E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
