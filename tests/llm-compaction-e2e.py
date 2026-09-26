#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: configurable LLM history compaction (ADR-125).

Flow (all phases hard-asserted, no graceful fallbacks):
1. Start cafe-bus + mock LLM + cafe-llm + cafe-store + cafe-agent-runtime + cafe-server
2. Create session with a runtime-owned TOML fixture agent
3. Phase A (message-count budget): config truncate/max_history_messages=2,
   chat msg-one/msg-two/msg-three, assert the last LLM request holds only the
   newest 2 non-system messages (msg-one compacted away, msg-three present)
4. Phase B (char budget): neutralize the count budget (max_history_messages=100),
   set max_history_chars=12, chat a long marker message, assert an older marker
   is compacted away from the last LLM request
5. Phase C (mode=none): disable compaction, chat again, assert the oldest
   marker is visible to the LLM again
6. Phase D (mode=summarize): re-enable budgets with summarize mode, chat,
   assert the backend first receives a summarization request and the
   generation request carries a "Prior conversation summary" system message
   while still compacting to the message budget

Usage:
    cargo build --release
    uv run tests/llm-compaction-e2e.py
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
            mock_requests.append({
                "model": model,
                "system_prompt": system_prompt,
                # Full message list so the test can assert compaction behavior.
                "messages": [{"role": m.get("role"), "content": m.get("content", "")}
                             for m in messages],
            })
        reply = f"Responding with {model}."
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


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def start_proc(cmd, env, logfile):
    return subprocess.Popen(cmd, env=env, stdout=open(logfile, "w"), stderr=subprocess.STDOUT)


def cli(bus_socket, *args):
    r = run([CLI, "--bus", bus_socket, "--server", f"http://localhost:{SERVER_PORT}",
             "--token", TOKEN, *args])
    assert r.returncode == 0, r.stderr
    return r.stdout.strip()


def publish_config(bus_socket, session_id, annotations):
    args = [CLI, "--bus", bus_socket, "publish", session_id, "--null",
            "--annotation", "config.type=runtime"]
    for key, value in annotations:
        args += ["--annotation", f"{key}={value}"]
    r = subprocess.run(args, capture_output=True, text=True)
    assert r.returncode == 0, f"publish config failed: {r.stderr}"


def chat(bus_socket, session_id, text):
    c = cli(bus_socket, "chat", session_id, text, "--timeout-secs", "60")
    chunks = [json.loads(line) for line in c.split("\n") if line.strip()]
    assert len(chunks) >= 1, f"expected SSE chunks for chat {text!r}, got none"
    return chunks


def last_request():
    with mock_lock:
        reqs = list(mock_requests)
    assert len(reqs) >= 1, "No LLM requests logged by mock server"
    return reqs[-1]


def non_system_messages(req):
    return [m for m in req["messages"] if m["role"] != "system"]


def all_content(req):
    return "\n".join(m["content"] for m in req["messages"])


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
        with open(os.path.join(agents_dir, "compaction-e2e.toml"), "w") as f:
            f.write(
                'name = "compaction-e2e"\n'
                'description = "llm compaction fixture"\n'
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
        env["OPENAI_MODEL"] = "mock-model"
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
            print("=== Create session ===", file=sys.stderr)
            r = subprocess.run([CLI, "--bus", bus_socket, "create-session", "--agent", "compaction-e2e"],
                               capture_output=True, text=True)
            assert r.returncode == 0, r.stderr
            session_id = r.stdout.strip()
            print(f"  session={session_id}", file=sys.stderr)

            # ── Phase A: message-count budget keeps only the newest 2 ──
            print("=== Phase A: truncate/max_history_messages=2 ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.model", "mock-model"),
                ("config.llm.compaction_mode", "truncate"),
                ("config.llm.max_history_messages", "2"),
            ])
            time.sleep(1)

            with mock_lock:
                mock_requests.clear()
            chat(bus_socket, session_id, "msg-one-alpha")
            chat(bus_socket, session_id, "msg-two-bravo")
            chat(bus_socket, session_id, "msg-three-charlie")

            req = last_request()
            non_sys = non_system_messages(req)
            assert len(non_sys) == 2, (
                f"Phase A: expected 2 non-system messages after compaction, got {len(non_sys)}: "
                f"{json.dumps(non_sys)}"
            )
            content = all_content(req)
            assert "msg-one-alpha" not in content, (
                f"Phase A: oldest message should have been compacted away: {content}"
            )
            assert "msg-three-charlie" in content, (
                f"Phase A: newest message must survive compaction: {content}"
            )
            print("  ✅ message-count compaction confirmed", file=sys.stderr)

            # ── Phase B: char budget compacts the old marker away ──
            print("=== Phase B: max_history_chars=12 ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.max_history_messages", "100"),
                ("config.llm.max_history_chars", "12"),
            ])
            time.sleep(1)

            chat(bus_socket, session_id, "marker-old-zzzz")
            chat(bus_socket, session_id, "marker-new-qqqq")

            req = last_request()
            content = all_content(req)
            assert "marker-new-qqqq" in content, (
                f"Phase B: newest message must survive char-budget compaction: {content}"
            )
            assert "marker-old-zzzz" not in content, (
                f"Phase B: old marker should have been compacted away by char budget: {content}"
            )
            print("  ✅ char-budget compaction confirmed", file=sys.stderr)

            # ── Phase C: mode=none disables compaction again ──
            print("=== Phase C: compaction_mode=none ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.compaction_mode", "none"),
            ])
            time.sleep(1)

            chat(bus_socket, session_id, "final-probe")

            req = last_request()
            content = all_content(req)
            assert "marker-old-zzzz" in content, (
                f"Phase C: with compaction disabled the old marker must be visible again: {content}"
            )
            print("  ✅ mode=none disables compaction confirmed", file=sys.stderr)

            # ── Phase D: summarize condenses the dropped prefix ──
            print("=== Phase D: compaction_mode=summarize ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.compaction_mode", "summarize"),
                ("config.llm.max_history_messages", "2"),
                ("config.llm.max_history_chars", "100000"),
            ])
            time.sleep(1)

            with mock_lock:
                phase_start = len(mock_requests)
            chat(bus_socket, session_id, "sum-probe-final")

            with mock_lock:
                new_reqs = list(mock_requests[phase_start:])
            assert len(new_reqs) >= 2, (
                f"Phase D: expected a summary request plus a generation request, got {len(new_reqs)}"
            )
            summary_reqs = [r for r in new_reqs
                            if r["system_prompt"].startswith("Summarize the following")]
            assert len(summary_reqs) >= 1, (
                f"Phase D: no summarization request reached the backend: "
                f"{json.dumps([r['system_prompt'][:60] for r in new_reqs])}"
            )
            req = new_reqs[-1]
            summaries = [m for m in req["messages"]
                         if m["role"] == "system" and "Prior conversation summary" in m["content"]]
            assert len(summaries) == 1, (
                f"Phase D: expected exactly one summary system message in the generation request: "
                f"{json.dumps(req['messages'])}"
            )
            non_sys = non_system_messages(req)
            assert len(non_sys) == 2, (
                f"Phase D: summarize must still compact to the message budget, got {len(non_sys)}: "
                f"{json.dumps(non_sys)}"
            )
            assert "sum-probe-final" in all_content(req), (
                f"Phase D: newest message must survive summarize compaction: {all_content(req)}"
            )
            print("  ✅ summarize compaction confirmed", file=sys.stderr)

            print("\n=== ALL LLM COMPACTION TESTS PASSED ===", file=sys.stderr)

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
