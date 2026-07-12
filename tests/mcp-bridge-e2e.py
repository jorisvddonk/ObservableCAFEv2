#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: cafe-mcp-bridge MCP server.

Tests tools/list and tools/call (inline web_fetch + meta tools + bus RPC).

Usage:
    cargo build --release
    uv run tests/mcp-bridge-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import uuid

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BRIDGE_BIN = os.path.join(RELEASE_DIR, "cafe-mcp-bridge")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")
KB_BIN = os.path.join(RELEASE_DIR, "cafe-knowledgebase")


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


class McpBridge:
    """Helper to interact with an MCP bridge process."""

    def __init__(self, args, env):
        self.proc = subprocess.Popen(
            args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, env=env, bufsize=0,
        )
        time.sleep(0.5)

    def send(self, method, params=None):
        """Send a JSON-RPC request and wait for the response."""
        msg = json.dumps({
            "jsonrpc": "2.0",
            "id": str(uuid.uuid4()),
            "method": method,
            "params": params or {},
        })
        self.proc.stdin.write((msg + "\n").encode())
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            return None
        return json.loads(line)

    def notify(self, method):
        """Send a JSON-RPC notification (no response expected)."""
        msg = json.dumps({"jsonrpc": "2.0", "method": method})
        self.proc.stdin.write((msg + "\n").encode())
        self.proc.stdin.flush()

    def close(self):
        self.proc.kill()
        self.proc.wait()


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI), ("cafe-mcp-bridge", BRIDGE_BIN)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["RUST_LOG"] = "error"

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        # ── Test 1: tools/list + web_fetch ──
        print("=== Test: tools/list + web_fetch ===", file=sys.stderr)
        bridge = McpBridge([BRIDGE_BIN, "--bus", bus_socket, "--tool", "web_fetch", "--tool", "kb_search"], env)

        resp = bridge.send("initialize")
        assert resp and resp.get("result", {}).get("serverInfo", {}).get("name") == "cafe-mcp-bridge"
        print("  initialized ✅", file=sys.stderr)

        bridge.notify("notifications/initialized")

        resp = bridge.send("tools/list")
        tools = resp.get("result", {}).get("tools", [])
        names = [t["name"] for t in tools]
        assert "web_fetch" in names and "kb_search" in names and len(tools) == 2
        print(f"  tools/list: {len(tools)} tools ✅", file=sys.stderr)

        resp = bridge.send("tools/call", {"name": "web_fetch", "arguments": {"url": "https://example.com"}})
        text = resp["result"]["content"][0]["text"]
        assert "Example Domain" in text
        print("  web_fetch ✅", file=sys.stderr)

        bridge.close()

        # ── Test 2: meta tools ──
        print("=== Test: meta tools ===", file=sys.stderr)
        bridge = McpBridge([BRIDGE_BIN, "--bus", bus_socket, "--meta", "--tool", "cafe_meta_ping"], env)

        bridge.send("initialize")
        bridge.notify("notifications/initialized")

        resp = bridge.send("tools/list")
        names = [t["name"] for t in resp["result"]["tools"]]
        assert "cafe_meta_ping" in names
        print("  meta tools listed ✅", file=sys.stderr)

        resp = bridge.send("tools/call", {"name": "cafe_meta_ping", "arguments": {}})
        text = resp["result"]["content"][0]["text"]
        print(f"  cafe_meta_ping response: {text[:100]}", file=sys.stderr)
        assert '"pong":true' in text or '"pong": true' in text, f"pong missing in: {text}"
        print("  cafe_meta_ping ✅", file=sys.stderr)
        bridge.close()

        # ── Test 3: kb_search via bus RPC ──
        print("=== Test: kb_search RPC ===", file=sys.stderr)
        store_db = os.path.join(tmpdir, "cafe.db")
        store_env = env.copy()
        store_env["CAFE_DB_PATH"] = store_db
        store_proc = subprocess.Popen(
            [STORE_BIN], env=store_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        time.sleep(1)

        kb_proc = subprocess.Popen(
            [KB_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        time.sleep(2)

        bridge = McpBridge([BRIDGE_BIN, "--bus", bus_socket, "--tool", "kb_search"], env)
        bridge.send("initialize")
        bridge.notify("notifications/initialized")

        resp = bridge.send("tools/call", {
            "name": "kb_search",
            "arguments": {"namespace": "geography", "query": "Sweden", "k": 3}
        })
        text = resp["result"]["content"][0]["text"]
        assert text, "kb_search returned empty"
        assert not resp["result"].get("isError"), f"kb_search returned error: {text[:200]}"
        print("  kb_search succeeded ✅", file=sys.stderr)

        bridge.close()
        kb_proc.kill()
        kb_proc.wait()
        store_proc.kill()
        store_proc.wait()

        bus_proc.kill()
        bus_proc.wait()

    print(file=sys.stderr)
    print("=== ALL MCP BRIDGE E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
