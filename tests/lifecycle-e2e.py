#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx"]
# ///
"""
Full lifecycle e2e test.

Tests: start services → create session → chat → verify LLM turn →
shutdown → restart → chat again (persistence) → verify → delete
session → verify deleted → shutdown.

Requires: cargo build --release, LLM backend running (port 8080).
"""

import httpx
import json
import os
import signal
import subprocess
import sys
import tempfile
import time

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")

BINARIES = {
    "bus": os.path.join(RELEASE_DIR, "cafe-bus"),
    "store": os.path.join(RELEASE_DIR, "cafe-store"),
    "llm": os.path.join(RELEASE_DIR, "cafe-llm"),
    "agent": os.path.join(RELEASE_DIR, "cafe-agent-runtime"),
    "server": os.path.join(RELEASE_DIR, "cafe-server"),
}
SERVER_PORT = os.environ.get("CAFE_SERVER_PORT", "49999")
SERVER_URL = f"http://localhost:{SERVER_PORT}"
TOKEN = "test-admin-token"


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


def require_binaries():
    missing = [k for k, v in BINARIES.items() if not os.path.exists(v)]
    if missing:
        print(f"Build release binaries first: cargo build --release -p {' -p '.join(missing)}",
              file=sys.stderr)
        sys.exit(1)


def start_all(env, tmpdir):
    """Start all services, return dict of process handles."""
    procs = {}
    for key, path in BINARIES.items():
        if key == "store":
            e = env.copy()
            e["CAFE_DB_PATH"] = os.path.join(tmpdir, "cafe.db")
        elif key == "server":
            e = env.copy()
            e["PORT"] = SERVER_PORT
        else:
            e = env
        log = open(os.path.join(tmpdir, f"{key}.log"), "w")
        procs[key] = subprocess.Popen([path], env=e, stdout=log, stderr=subprocess.STDOUT)
        time.sleep(0.5)
    # Wait extra for agent to discover sessions
    time.sleep(2)
    return procs


def stop_all(procs, tmpdir):
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


def create_session(token, agent="default"):
    r = httpx.post(
        f"{SERVER_URL}/api/sessions",
        json={"agent_id": agent},
        headers={"Authorization": f"Bearer {token}"},
    )
    r.raise_for_status()
    data = r.json()
    print(f"  session={data['id']} agent={data['agent_id']}", file=sys.stderr)
    return data["id"]


def delete_session(token, session_id):
    r = httpx.delete(
        f"{SERVER_URL}/api/sessions/{session_id}",
        headers={"Authorization": f"Bearer {token}"},
    )
    r.raise_for_status()


def list_sessions(token):
    r = httpx.get(
        f"{SERVER_URL}/api/sessions",
        headers={"Authorization": f"Bearer {token}"},
    )
    r.raise_for_status()
    return [s["session_id"] for s in r.json()]


def send_chat(token, session_id, message, timeout=60):
    """Send a chat message, return SSE events as a list of chunks.
    Runs in a thread so we can enforce a hard wall-clock timeout."""
    from concurrent.futures import ThreadPoolExecutor

    def _read():
        r = httpx.post(
            f"{SERVER_URL}/api/sessions/{session_id}/chat",
            json={"content": message},
            headers={"Authorization": f"Bearer {token}"},
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
            return pool.submit(_read).result(timeout=timeout)
        except Exception:
            raise TimeoutError(f"Chat timed out after {timeout}s")


def get_history(token, session_id):
    r = httpx.get(
        f"{SERVER_URL}/api/sessions/{session_id}/history",
        headers={"Authorization": f"Bearer {token}"},
    )
    r.raise_for_status()
    return r.json().get("chunks", [])


def main():
    require_binaries()

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        admin_log = os.path.join(tmpdir, "admin.log")

        db_path = os.path.join(tmpdir, "cafe.db")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["LLM_BACKEND"] = "openai"
        env["OPENAI_URL"] = "http://localhost:8080"
        env["OPENAI_MODEL"] = "gemma3:1b"
        env["CAFE_ADMIN_TOKEN"] = TOKEN
        env["CAFE_DB_PATH"] = db_path

        # ── Phase 1: First run ──
        print("=== Phase 1: First run ===", file=sys.stderr)
        procs = start_all(env, tmpdir)

        # Save admin token
        time.sleep(1)
        admin_token = TOKEN
        print(f"  token={admin_token}", file=sys.stderr)

        try:
            # Create session
            print("=== Create session ===", file=sys.stderr)
            session_id = create_session(admin_token)

            # Send a message and verify LLM response
            print("=== Chat ===", file=sys.stderr)
            chunks = send_chat(admin_token, session_id, "Say hello in one word.")
            print(f"  {len(chunks)} SSE chunks", file=sys.stderr)

            # Debug: show all chunks
            for i, c in enumerate(chunks[:10]):
                print(f"  chunk[{i}]: type={c.get('content_type')} role={c.get('annotations',{}).get('chat.role')} content={str(c.get('content',''))[:80]}", file=sys.stderr)

            # Check that there's an assistant response
            assistant = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            content = next((c.get("content", "") for c in assistant if c.get("content")), "")
            print(f"  assistant: {content[:100]}", file=sys.stderr)
            assert len(assistant) > 0, f"No assistant chunks in SSE (got {len(chunks)} chunks)"

            # Verify history
            history = get_history(admin_token, session_id)
            user_msgs = [c for c in history if c.get("annotations", {}).get("chat.role") == "user"]
            asst_msgs = [c for c in history if c.get("annotations", {}).get("chat.role") == "assistant"]
            assert len(user_msgs) >= 1, "No user message in history"
            assert len(asst_msgs) >= 1, "No assistant message in history"
            print(f"  history: {len(user_msgs)} user, {len(asst_msgs)} assistant", file=sys.stderr)

        except:
            dump_logs(tmpdir)
            raise
        finally:
            # Shutdown
            print("=== Shutdown ===", file=sys.stderr)
            stop_all(procs, tmpdir)

        # ── Phase 2: Restart and verify persistence ──
        print("=== Phase 2: Restart ===", file=sys.stderr)
        procs = start_all(env, tmpdir)

        try:
            # Verify session still exists (wait for restore to complete)
            time.sleep(3)
            sessions = list_sessions(admin_token)
            assert session_id in sessions, f"Session {session_id} lost on restart"
            print(f"  session persisted: {session_id}", file=sys.stderr)

            # Verify history still intact
            history = get_history(admin_token, session_id)
            print(f"  history: {len(history)} chunks", file=sys.stderr)
            for i, c in enumerate(history[:5]):
                print(f"    [{i}] type={c.get('content_type')} role={c.get('annotations',{}).get('chat.role')} prod={c.get('producer','')[:30]}", file=sys.stderr)
            asst_msgs = [c for c in history if c.get("annotations", {}).get("chat.role") == "assistant"]
            assert len(asst_msgs) >= 1, f"No assistant messages after restart (history has {len(history)} chunks)"

            # Send another message
            print("=== Chat (post-restart) ===", file=sys.stderr)
            chunks = send_chat(admin_token, session_id, "Say goodbye in one word.")
            print(f"  {len(chunks)} SSE chunks", file=sys.stderr)
            asst = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
            content = next((c.get("content", "") for c in asst if c.get("content")), "")
            assert len(content.strip()) > 0, f"Empty assistant response ({len(chunks)} chunks)"
            print(f"  assistant: {content[:100]}", file=sys.stderr)

            # Delete session
            print("=== Delete session ===", file=sys.stderr)
            delete_session(admin_token, session_id)

            # Verify deleted
            sessions = list_sessions(admin_token)
            assert session_id not in sessions, f"Session {session_id} still exists after delete"
            print("  session deleted", file=sys.stderr)

        except:
            dump_logs(tmpdir)
            raise
        finally:
            print("=== Final shutdown ===", file=sys.stderr)
            stop_all(procs, tmpdir)

    print(file=sys.stderr)
    print("=== ALL LIFECYCLE TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
