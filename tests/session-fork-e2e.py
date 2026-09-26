#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: session forking.

Flow:
1. Start cafe-bus + mock LLM + cafe-llm + cafe-agent-runtime + cafe-store + cafe-server
2. Create a session and exchange a couple of chat messages so it has history
3. Fork the session
4. Verify the fork's history is identical to the parent's (verbatim copy,
   timestamps preserved), parent is unmodified, and fork.parent_id provenance is present
5. Verify forking alone does not trigger extra LLM invocations (no active replay),
   and that the fork responds to a fresh message after creation

Usage:
    cargo build --release
    uv run tests/session-fork-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")
LLM_BIN = os.path.join(RELEASE_DIR, "cafe-llm")
AGENT_BIN = os.path.join(RELEASE_DIR, "cafe-agent-runtime")
SERVER_BIN = os.path.join(RELEASE_DIR, "cafe-server")

MOCK_PORT = 49993
SERVER_PORT = 49992
TOKEN = "test-admin-token"

mock_requests = []
mock_lock = threading.Lock()


class MockLLMHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length).decode())
        model = body.get("model", "unknown")
        messages = body.get("messages", [])
        system_prompt = next((m["content"] for m in messages if m.get("role") == "system"), "")
        with mock_lock:
            mock_requests.append({"model": model, "system_prompt": system_prompt})
        reply = f"Responding with {model}. System: {system_prompt[:40]}..."
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        for word in reply.split():
            chunk = {"id": "mock-0", "object": "chat.completion.chunk",
                     "choices": [{"delta": {"content": word + " "}, "index": 0}]}
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()
        final = {"id": "mock-0", "object": "chat.completion.chunk",
                 "choices": [{"delta": {}, "finish_reason": "stop", "index": 0}]}
        self.wfile.write(f"data: {json.dumps(final)}\n\n".encode())
        self.wfile.write("data: [DONE]\n\n".encode())
        self.wfile.flush()

    def log_message(self, format, *args):
        pass


def run_mock_server():
    server = HTTPServer(("0.0.0.0", MOCK_PORT), MockLLMHandler)
    server.timeout = 0.5
    while getattr(server, "running", True):
        server.handle_request()


def start_proc(cmd, env, logfile):
    return subprocess.Popen(cmd, env=env, stdout=open(logfile, "w"), stderr=subprocess.STDOUT)


def cli(bus_socket, *args):
    r = subprocess.run([CLI, "--bus", bus_socket, "--server", f"http://localhost:{SERVER_PORT}",
                        "--token", TOKEN, *args], capture_output=True, text=True)
    assert r.returncode == 0, f"cli {args} failed: {r.stderr}"
    return r.stdout.strip()


def history_chunks(bus_socket, session_id):
    out = cli(bus_socket, "history", session_id)
    chunks = []
    for line in out.split("\n"):
        if line.strip():
            chunks.append(json.loads(line))
    return chunks


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-store", STORE_BIN), ("cafe-llm", LLM_BIN),
                        ("cafe-agent-runtime", AGENT_BIN), ("cafe-server", SERVER_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        # A TOML fixture agent owned by cafe-agent-runtime (unique name so no
        # JS agent shadows it); mirrors `default`'s llm-on-user-message step.
        agents_dir = os.path.join(tmpdir, "agents")
        os.mkdir(agents_dir)
        with open(os.path.join(agents_dir, "fork-e2e.toml"), "w") as f:
            f.write(
                'name = "fork-e2e"\n'
                'description = "session fork fixture"\n'
                "background = false\n"
                "allows_reload = true\n"
                "persists_state = true\n\n"
                "[[steps]]\n"
                'id = "llm"\n'
                'type = "llm"\n'
                'trigger = "user_message"\n'
            )

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["CAFE_DB_PATH"] = db_path
        env["CAFE_ADMIN_TOKEN"] = TOKEN
        env["PORT"] = str(SERVER_PORT)
        env["LLM_BACKEND"] = "openai"
        env["OPENAI_URL"] = f"http://localhost:{MOCK_PORT}/v1"
        env["OPENAI_MODEL"] = "gemma3:1b"
        env["ObservableCAFE_AGENT_SEARCH_PATHS"] = agents_dir
        env["CAFE_AGENT_PATHS"] = agents_dir
        env.pop("CAFE_JS_AGENT_PATHS", None)

        mock_thread = threading.Thread(target=run_mock_server, daemon=True)
        mock_thread.start()
        time.sleep(0.5)

        print("=== Starting services ===", file=sys.stderr)
        procs = {}
        for key, path in [
            ("bus", BUS_BIN), ("store", STORE_BIN), ("llm", LLM_BIN),
            ("agent", AGENT_BIN), ("server", SERVER_BIN),
        ]:
            log = os.path.join(tmpdir, f"{key}.log")
            procs[key] = start_proc([path], env, log)
            time.sleep(1)
        time.sleep(2)

        try:
            # ── Create parent session and exchange messages ──
            print("=== Create parent session ===", file=sys.stderr)
            parent_id = cli(bus_socket, "create-session", "--agent", "fork-e2e")
            print(f"  parent={parent_id}", file=sys.stderr)

            mock_requests.clear()
            cli(bus_socket, "chat", parent_id, "first message", "--timeout-secs", "60")
            cli(bus_socket, "chat", parent_id, "second message", "--timeout-secs", "60")

            # Let the parent's pipeline attachment (config/schema chunks) settle
            # so the parent history is stable before we snapshot it for forking.
            time.sleep(2)
            parent_history = history_chunks(bus_socket, parent_id)
            # Verify the parent history is stable (not still growing from attach).
            time.sleep(1)
            parent_history_again = history_chunks(bus_socket, parent_id)
            assert len(parent_history_again) == len(parent_history), (
                f"parent history not stable: {len(parent_history)} -> {len(parent_history_again)}"
            )
            parent_history = parent_history_again
            assert len(parent_history) >= 4, f"expected parent history, got {len(parent_history)}"
            parent_user_chunks = [c for c in parent_history if c.get("annotations", {}).get("chat.role") == "user"]
            assert len(parent_user_chunks) >= 2, "expected at least 2 user messages in parent history"
            print(f"  parent history: {len(parent_history)} chunks", file=sys.stderr)

            # Record the number of LLM invocations so far (parent chat) for active-replay check
            with mock_lock:
                reqs_before_fork = len(mock_requests)
            print(f"  LLM invocations before fork: {reqs_before_fork}", file=sys.stderr)

            # ── Fork the session ──
            print("=== Fork session ===", file=sys.stderr)
            fork_id = cli(bus_socket, "fork-session", parent_id)
            print(f"  fork={fork_id}", file=sys.stderr)
            assert fork_id != parent_id, "fork id must differ from parent"
            time.sleep(1)

            # Forking alone must not trigger extra LLM invocations (no active replay)
            with mock_lock:
                reqs_after_fork = len(mock_requests)
            assert reqs_after_fork == reqs_before_fork, (
                f"fork triggered active replay: invocations went from {reqs_before_fork} to {reqs_after_fork}"
            )
            print("  ✅ no active replay on fork", file=sys.stderr)

            # ── Verify fork is a verbatim copy of the parent ──
            # get_history returns history + retained transient chunks, so the
            # fork may have extra chunks (provenance, attach-published config/
            # schema) interleaved with retained ones. Assert that every parent
            # chunk appears in the fork with identical id/content/timestamp.
            fork_history = history_chunks(bus_socket, fork_id)
            fork_by_id = {c["id"]: c for c in fork_history}
            assert len(fork_history) > len(parent_history), (
                f"fork history should be >= parent: parent={len(parent_history)} fork={len(fork_history)}"
            )
            for parent_chunk in parent_history:
                fc = fork_by_id.get(parent_chunk["id"])
                assert fc is not None, f"parent chunk {parent_chunk['id']} missing from fork"
                assert fc["content"] == parent_chunk["content"], f"chunk {parent_chunk['id']} content mismatch"
                assert fc["timestamp"] == parent_chunk["timestamp"], f"chunk {parent_chunk['id']} timestamp mismatch"
            print("  ✅ fork contains a verbatim copy of all parent chunks (ids, content, timestamps)", file=sys.stderr)

            # Provenance chunk: search for it in the fork history.
            prov_idx = None
            for i, c in enumerate(fork_history):
                if c.get("annotations", {}).get("fork.parent_id") == parent_id:
                    prov_idx = i
                    break
            assert prov_idx is not None, "fork.parent_id provenance annotation missing"
            assert fork_history[prov_idx].get("content_type") == "null", "provenance chunk should be null"
            print("  ✅ fork.parent_id provenance present", file=sys.stderr)

            # Parent is not modified
            parent_history_after = history_chunks(bus_socket, parent_id)
            assert len(parent_history_after) == len(parent_history), "parent history changed after fork"
            print("  ✅ parent unmodified", file=sys.stderr)

            # ── Fork responds to a fresh message after creation ──
            print("=== Fork responds to fresh message ===", file=sys.stderr)
            mock_requests.clear()
            cli(bus_socket, "chat", fork_id, "continue the conversation", "--timeout-secs", "60")
            with mock_lock:
                reqs_fork_chat = len(mock_requests)
            assert reqs_fork_chat >= 1, "fork did not produce an LLM invocation for a fresh message"
            print("  ✅ fork responds to a fresh message", file=sys.stderr)

            # Fork history now has one more user+assistant pair (the fresh message)
            fork_history_after = history_chunks(bus_socket, fork_id)
            assert len(fork_history_after) > len(fork_history), "fork history did not grow after chat"

            print("\n=== ALL SESSION FORK TESTS PASSED ===", file=sys.stderr)

        finally:
            for key in ["server", "agent", "llm", "store", "bus"]:
                p = procs.pop(key, None)
                if p:
                    p.terminate()
                    try:
                        p.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        p.kill()
                        p.wait()


if __name__ == "__main__":
    main()
