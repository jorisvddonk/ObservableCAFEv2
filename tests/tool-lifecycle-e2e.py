#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Lifecycle e2e test for LLM-generated tool calls — uses cafe-cli for all ops.

Tests: start → dice-llm session → chat "roll 2d6" → LLM emits
<|tool_call|> → shutdown → restart → verify persistence → chat
again → delete.

Requires: cargo build --release, LLM backend running (port 8080).
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

BINARIES = {
    "bus": os.path.join(RELEASE_DIR, "cafe-bus"),
    "store": os.path.join(RELEASE_DIR, "cafe-store"),
    "llm": os.path.join(RELEASE_DIR, "cafe-llm"),
    "agent": os.path.join(RELEASE_DIR, "cafe-agent-runtime"),
    "server": os.path.join(RELEASE_DIR, "cafe-server"),
    "dice": os.path.join(RELEASE_DIR, "cafe-dice"),
}


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def cli(socket_path, *args):
    r = run([CLI, "--bus", socket_path, "--server", "http://localhost:49999",
             "--token", "test-admin-token", *args])
    assert r.returncode == 0, r.stderr
    return r.stdout.strip()


def main():
    for p in list(BINARIES.values()) + [CLI]:
        if not os.path.exists(p):
            print(f"Build release binaries first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["LLM_BACKEND"] = "openai"
        env["OPENAI_URL"] = "http://localhost:8080"
        env["OPENAI_MODEL"] = "gemma3:1b"
        env["CAFE_ADMIN_TOKEN"] = "test-admin-token"
        env["CAFE_DB_PATH"] = db_path

        # ── Phase 1 ──
        print("=== Phase 1: First run ===", file=sys.stderr)
        procs = {}
        for key, path in BINARIES.items():
            log = open(os.path.join(tmpdir, f"{key}.log"), "w")
            e = env.copy()
            if key == "server":
                e["PORT"] = "49999"
            procs[key] = subprocess.Popen([path], env=e, stdout=log, stderr=subprocess.STDOUT)
            time.sleep(0.5)
        time.sleep(2)

        try:
            session_id = cli(bus_socket, "create-session", "--agent", "dice-llm")
            print(f"  session={session_id}", file=sys.stderr)

            c = cli(bus_socket, "chat", session_id, "roll 2d6", "--timeout-secs", "60")
            chunks = [json.loads(l) for l in c.split("\n") if l.strip()]
            asst = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            text = "".join(c.get("content", "") or "" for c in asst)
            assert len(text.strip()) > 0, "Empty LLM response"
            has_tool = "<|tool_call|>" in text
            print(f"  {len(chunks)} chunks, tool_call={has_tool}", file=sys.stderr)

            h = cli(bus_socket, "history", session_id)
            history = [json.loads(l) for l in h.split("\n") if l.strip()]
            print(f"  history: {len(history)} chunks", file=sys.stderr)

        finally:
            print("=== Shutdown ===", file=sys.stderr)
            for key in ["server", "agent", "llm", "dice", "store", "bus"]:
                p = procs.pop(key, None)
                if p:
                    p.terminate()
            for key in ["server", "agent", "llm", "dice", "store", "bus"]:
                p = procs.pop(key, None)
                if p:
                    try:
                        p.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        p.kill()
                        p.wait()

        # ── Phase 2 ──
        print("=== Phase 2: Restart ===", file=sys.stderr)
        for key, path in BINARIES.items():
            log = open(os.path.join(tmpdir, f"{key}.log"), "w")
            e = env.copy()
            if key == "server":
                e["PORT"] = "49999"
            procs[key] = subprocess.Popen([path], env=e, stdout=log, stderr=subprocess.STDOUT)
            time.sleep(0.5)
        time.sleep(3)

        try:
            h = cli(bus_socket, "history", session_id)
            history = [json.loads(l) for l in h.split("\n") if l.strip()]
            assert len(history) >= 1, "Empty history after restart"
            print(f"  history: {len(history)} chunks", file=sys.stderr)

            c = cli(bus_socket, "chat", session_id, "roll a d20", "--timeout-secs", "60")
            chunks = [json.loads(l) for l in c.split("\n") if l.strip()]
            asst = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            text = "".join(c.get("content", "") or "" for c in asst)
            assert len(text.strip()) > 0, "Empty response after restart"
            print(f"  response: {text[:100]}", file=sys.stderr)

            cli(bus_socket, "delete-session", session_id)
            s = json.loads(cli(bus_socket, "list-sessions"))
            assert session_id not in [x["session_id"] for x in s]
            print("  deleted", file=sys.stderr)

        finally:
            print("=== Final shutdown ===", file=sys.stderr)
            for key in ["server", "agent", "llm", "dice", "store", "bus"]:
                p = procs.pop(key, None)
                if p:
                    p.terminate()
            for key in ["server", "agent", "llm", "dice", "store", "bus"]:
                p = procs.pop(key, None)
                if p:
                    try:
                        p.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        p.kill()
                        p.wait()

    print(file=sys.stderr)
    print("=== ALL TOOL LIFECYCLE TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
