#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: tool calling pipeline via cafe-dice (dice roller).

Tests the round-trip: user message -> pipeline -> dice-detector.invoke RPC
-> cafe-dice parses !roll -> publishes tool.call -> tool-executor dispatches
dice.roll -> cafe-dice rolls -> subscriber catches jsonrpc.response.

Usage:
    cargo build --release
    uv run tests/tool-calling-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")
AGENT_BIN = os.path.join(RELEASE_DIR, "cafe-agent-runtime")
DICE_BIN = os.path.join(RELEASE_DIR, "cafe-dice")


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-store", STORE_BIN),
                       ("cafe-agent-runtime", AGENT_BIN),
                       ("cafe-cli", CLI), ("cafe-dice", DICE_BIN)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        store_db = os.path.join(tmpdir, "cafe.db")
        agent_log = os.path.join(tmpdir, "agent.log")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-store ===", file=sys.stderr)
        store_env = env.copy()
        store_env["CAFE_DB_PATH"] = store_db
        store_proc = subprocess.Popen([STORE_BIN], env=store_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-dice ===", file=sys.stderr)
        dice_proc = subprocess.Popen([DICE_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-agent-runtime ===", file=sys.stderr)
        agent_env = env.copy()
        agent_env["CAFE_DB_PATH"] = store_db
        agent_proc = subprocess.Popen(
            [AGENT_BIN], env=agent_env, stdout=subprocess.DEVNULL, stderr=open(agent_log, "w"),
        )
        time.sleep(2)

        try:
            # Create dice session
            print("=== Create dice session ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "dice"])
            assert r.returncode == 0
            session_id = r.stdout.strip()
            print(f"  session={session_id}", file=sys.stderr)
            time.sleep(1)

            # Publish roll command
            print("=== Publish !roll 2d6 ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "publish", session_id, "--text", "!roll 2d6"])
            assert r.returncode == 0
            time.sleep(1)

            # Subscribe (retained buffer serves the transient RPC response)
            print("=== Subscribe for result ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "subscribe", session_id, "--timeout-secs", "6"])
            assert r.returncode == 0

            # Parse chunks looking for the dice.roll result
            result = None
            for line in r.stdout.strip().split("\n"):
                if not line.strip():
                    continue
                try:
                    chunk = json.loads(line)
                    rpc_resp = chunk.get("annotations", {}).get("jsonrpc.response")
                    if rpc_resp and rpc_resp.get("result", {}).get("result") is not None:
                        result = rpc_resp["result"]["result"]
                        break
                except (json.JSONDecodeError, AttributeError):
                    continue

            assert result is not None, f"no dice.roll result found"
            assert isinstance(result, (int, float)), f"result should be a number, got {type(result)}"
            assert 2 <= result <= 12, f"2d6 should be 2-12, got {result}"
            print(f"  dice roll result={result} ✅", file=sys.stderr)

            # Clean up
            run([CLI, "--bus", bus_socket, "delete-session", session_id])

        finally:
            for p in [bus_proc, store_proc, dice_proc, agent_proc]:
                p.kill()
            for p in [bus_proc, store_proc, dice_proc, agent_proc]:
                p.wait()

    print(file=sys.stderr)
    print("=== ALL TOOL CALLING TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
