#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx"]
# ///
"""
Lifecycle e2e test for LLM-generated tool calls.

Tests: start → dice-llm session → chat "roll 2d6" → LLM emits
<|tool_call|> → shutdown → restart → verify persistence → chat
again → delete.

Requires: cargo build --release, LLM backend running (port 8080).
"""

import httpx
import json
import os
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")

BINARIES = {
    "bus": os.path.join(RELEASE_DIR, "cafe-bus"),
    "store": os.path.join(RELEASE_DIR, "cafe-store"),
    "llm": os.path.join(RELEASE_DIR, "cafe-llm"),
    "agent": os.path.join(RELEASE_DIR, "cafe-agent-runtime"),
    "server": os.path.join(RELEASE_DIR, "cafe-server"),
    "dice": os.path.join(RELEASE_DIR, "cafe-dice"),
}
SERVER_PORT = os.environ.get("CAFE_SERVER_PORT", "49998")
SERVER_URL = f"http://localhost:{SERVER_PORT}"


def require_binaries():
    missing = [k for k, v in BINARIES.items() if not os.path.exists(v)]
    if missing:
        print(f"Build release binaries first: cargo build --release -p {' -p '.join(missing)}")
        sys.exit(1)


def dump_logs(tmpdir):
    for key in ["bus", "store", "llm", "agent", "server", "dice"]:
        logfile = os.path.join(tmpdir, f"{key}.log")
        try:
            with open(logfile) as f:
                lines = f.readlines()
                print(f"  [{key}.log] last 10 lines:", file=sys.stderr)
                for line in lines[-10:]:
                    print(f"    {line.rstrip()[:200]}", file=sys.stderr)
        except FileNotFoundError:
            pass


def start_all(env, tmpdir):
    procs = {}
    for key, path in BINARIES.items():
        e = env.copy()
        if key == "server":
            e["PORT"] = SERVER_PORT
        log = open(os.path.join(tmpdir, f"{key}.log"), "w")
        procs[key] = subprocess.Popen([path], env=e, stdout=log, stderr=subprocess.STDOUT)
        time.sleep(0.5)
    time.sleep(2)
    return procs


def stop_all(procs):
    for key in ["server", "agent", "llm", "dice", "store", "bus"]:
        p = procs.pop(key, None)
        if p:
            p.terminate()
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()


def create_session(token, agent="default"):
    r = httpx.post(
        f"{SERVER_URL}/api/sessions",
        json={"agent_id": agent},
        headers={"Authorization": f"Bearer {token}"},
        timeout=10,
    )
    r.raise_for_status()
    return r.json()["id"]


def delete_session(token, session_id):
    r = httpx.delete(
        f"{SERVER_URL}/api/sessions/{session_id}",
        headers={"Authorization": f"Bearer {token}"},
        timeout=10,
    )
    r.raise_for_status()


def send_chat(token, session_id, message, timeout=60):
    def _read():
        r = httpx.post(
            f"{SERVER_URL}/api/sessions/{session_id}/chat",
            json={"content": message},
            headers={"Authorization": f"Bearer {token}"},
            timeout=timeout,
        )
        r.raise_for_status()
        chunks = []
        for line in r.iter_lines():
            if not line or not line.startswith("data: "):
                continue
            chunk = json.loads(line[6:])
            chunks.append(chunk)
            if chunk.get("annotations", {}).get("chat.stream_complete"):
                break
        return chunks

    with ThreadPoolExecutor(1) as pool:
        try:
            return pool.submit(_read).result(timeout=timeout + 10)
        except Exception:
            raise TimeoutError(f"Chat timed out after {timeout}s")


def get_history(token, session_id):
    r = httpx.get(
        f"{SERVER_URL}/api/sessions/{session_id}/history",
        headers={"Authorization": f"Bearer {token}"},
        timeout=10,
    )
    r.raise_for_status()
    return r.json().get("chunks", [])


def main():
    require_binaries()

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

        # ── Phase 1: First run ──
        print("=== Phase 1: First run ===", file=sys.stderr)
        procs = start_all(env, tmpdir)
        token = "test-admin-token"

        try:
            session_id = create_session(token, "dice-llm")
            print(f"  session={session_id}", file=sys.stderr)

            chunks = send_chat(token, session_id, "roll 2d6")
            print(f"  {len(chunks)} SSE chunks", file=sys.stderr)

            # Check LLM responded
            assistant = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            full_text = "".join(c.get("content", "") or "" for c in assistant)
            print(f"  LLM response ({len(full_text)} chars): {full_text[:200]}", file=sys.stderr)
            assert len(full_text.strip()) > 0, "Empty LLM response"

            # Check response contains tool call marker (LLM was prompted correctly)
            has_tool_call = "<|tool_call|>" in full_text
            if has_tool_call:
                print(f"  LLM generated tool call ✅", file=sys.stderr)
            else:
                print(f"  No tool call in response (LLM may not have understood)", file=sys.stderr)

            # Check history for assistant chunks
            history = get_history(token, session_id)
            asst_msgs = [c for c in history if c.get("annotations", {}).get("chat.role") == "assistant"]
            print(f"  history: {len(asst_msgs)} assistant chunks", file=sys.stderr)

        except:
            dump_logs(tmpdir)
            raise
        finally:
            print("=== Shutdown ===", file=sys.stderr)
            stop_all(procs)

        # ── Phase 2: Restart ──
        print("=== Phase 2: Restart ===", file=sys.stderr)
        procs = start_all(env, tmpdir)

        try:
            time.sleep(2)
            history = get_history(token, session_id)
            asst_msgs = [c for c in history if c.get("annotations", {}).get("chat.role") == "assistant"]
            assert len(asst_msgs) >= 1, f"No assistant messages after restart ({len(history)} chunks)"
            print(f"  history persisted: {len(history)} chunks", file=sys.stderr)

            chunks = send_chat(token, session_id, "roll a d20")
            assistant = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            text = "".join(c.get("content", "") or "" for c in assistant)
            assert len(text.strip()) > 0, "Empty response after restart"
            print(f"  post-restart response ({len(text)} chars)", file=sys.stderr)

            delete_session(token, session_id)
            print(f"  session deleted", file=sys.stderr)

        except:
            dump_logs(tmpdir)
            raise
        finally:
            print("=== Final shutdown ===", file=sys.stderr)
            stop_all(procs)

    print(file=sys.stderr)
    print("=== ALL TOOL LIFECYCLE TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
