#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Full lifecycle e2e test — uses cafe-cli for all bus/HTTP operations.

Tests: start services → create session → chat → verify LLM turn →
shutdown → restart → chat again (persistence) → verify → delete
session → verify deleted → shutdown.

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
CLI_ARGS = ["--bus"]

BINARIES = {
    "bus": os.path.join(RELEASE_DIR, "cafe-bus"),
    "store": os.path.join(RELEASE_DIR, "cafe-store"),
    "llm": os.path.join(RELEASE_DIR, "cafe-llm"),
    "agent": os.path.join(RELEASE_DIR, "cafe-agent-runtime"),
    "server": os.path.join(RELEASE_DIR, "cafe-server"),
}


def dump_logs(tmpdir):
    for key in ["bus", "store", "llm", "agent", "server"]:
        logfile = os.path.join(tmpdir, f"{key}.log")
        try:
            with open(logfile) as f:
                lines = f.readlines()
                print(f"  [{key}.log] last 15 lines:", file=sys.stderr)
                for line in lines[-15:]:
                    print(f"    {line.rstrip()[:200]}", file=sys.stderr)
        except FileNotFoundError:
            pass


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def start_all(env, tmpdir):
    procs = {}
    for key, path in BINARIES.items():
        e = env.copy()
        if key == "server":
            e["PORT"] = "49999"
        log = open(os.path.join(tmpdir, f"{key}.log"), "w")
        procs[key] = subprocess.Popen([path], env=e, stdout=log, stderr=subprocess.STDOUT)
        time.sleep(0.5)
    time.sleep(2)
    return procs


def stop_all(procs):
    for key in ["server", "agent", "llm", "store", "bus"]:
        p = procs.pop(key, None)
        if p:
            p.terminate()
    for key in ["server", "agent", "llm", "store", "bus"]:
        p = procs.pop(key, None)
        if p:
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()


def cli(socket_path, *args):
    r = run([CLI, "--bus", socket_path, "--server", "http://localhost:49999",
             "--token", "test-admin-token", *args])
    assert r.returncode == 0, r.stderr
    return r.stdout.strip()


def main():
    for path in list(BINARIES.values()) + [CLI]:
        if not os.path.exists(path):
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
        procs = start_all(env, tmpdir)

        try:
            # Verify list-sessions works on a fresh bus
            s = json.loads(cli(bus_socket, "list-sessions"))
            print(f"  initial sessions: {len(s)}", file=sys.stderr)

            session_id = cli(bus_socket, "create-session", "--agent", "default")
            print(f"  session={session_id}", file=sys.stderr)

            chunks_raw = cli(bus_socket, "chat", session_id, "Say hello in one word.", "--timeout-secs", "60")
            chunks = [json.loads(l) for l in chunks_raw.strip().split("\n") if l.strip()]
            print(f"  {len(chunks)} SSE chunks", file=sys.stderr)
            asst = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            text = "".join(c.get("content", "") or "" for c in asst)
            assert len(text.strip()) > 0, "Empty assistant response"
            print(f"  assistant: {text[:100]}", file=sys.stderr)

            # Debug: check sessions and try history
            s = json.loads(cli(bus_socket, "list-sessions"))
            ids = [x["session_id"] for x in s]
            print(f"  sessions: {ids}", file=sys.stderr)
            assert session_id in ids, f"Session {session_id} not in bus sessions: {ids}"

            history_raw = cli(bus_socket, "history", session_id)
            history = [json.loads(l) for l in history_raw.strip().split("\n") if l.strip()]
            asst_msgs = [c for c in history if c.get("annotations", {}).get("chat.role") == "assistant"]
            assert len(asst_msgs) >= 1
            print(f"  history: {len(asst_msgs)} assistant", file=sys.stderr)

        except:
            dump_logs(tmpdir)
            raise
        finally:
            print("=== Shutdown ===", file=sys.stderr)
            stop_all(procs)

        # Verify DB has data before restart
        import sqlite3
        db_size = os.path.getsize(db_path) if os.path.exists(db_path) else 0
        print(f"  DB file: {db_size} bytes", file=sys.stderr)
        if db_size > 0:
            conn = sqlite3.connect(db_path)
            sessions = conn.execute("SELECT COUNT(*) FROM sessions").fetchone()[0]
            chunks = conn.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
            conn.close()
            print(f"  DB has {sessions} sessions, {chunks} chunks", file=sys.stderr)

        # ── Phase 2: Restart ──
        print("=== Phase 2: Restart ===", file=sys.stderr)
        procs = start_all(env, tmpdir)

        try:
            time.sleep(5)
            history_raw = cli(bus_socket, "history", session_id)
            history = [json.loads(l) for l in history_raw.strip().split("\n") if l.strip()]
            assert len(history) >= 1, "Empty history after restart"
            print(f"  history: {len(history)} chunks", file=sys.stderr)

            chunks_raw = cli(bus_socket, "chat", session_id, "Say goodbye in one word.", "--timeout-secs", "60")
            chunks = [json.loads(l) for l in chunks_raw.strip().split("\n") if l.strip()]
            asst = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            text = "".join(c.get("content", "") or "" for c in asst)
            assert len(text.strip()) > 0, "Empty response after restart"
            print(f"  assistant: {text[:100]}", file=sys.stderr)

            cli(bus_socket, "delete-session", session_id)
            sessions = json.loads(cli(bus_socket, "list-sessions"))
            assert session_id not in [s["session_id"] for s in sessions]
            print(f"  session deleted", file=sys.stderr)

        except:
            dump_logs(tmpdir)
            raise
        finally:
            print("=== Final shutdown ===", file=sys.stderr)
            stop_all(procs)

    print(file=sys.stderr)
    print("=== ALL LIFECYCLE TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
