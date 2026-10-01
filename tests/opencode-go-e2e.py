#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: cafe-llm OpenCode Go backend (direct HTTP API with an API key)
and per-session backend selection.

Proves cafe-llm talks to the OpenCode Go gateway directly (no `opencode` app),
that `config.llm.backend` selects the provider per session, that each model is
routed to the correct wire protocol, and that auth + the required
`x-opencode-session` header are correct.

Flow (every phase hard-asserted, no graceful fallbacks):
1. Start cafe-bus + mock gateway + cafe-llm + cafe-store + cafe-agent-runtime +
   cafe-server. Process default backend is the local OpenAI-compatible mock;
   OpenCode Go is selected per session.
2. Create a session with a runtime-owned TOML fixture agent
3. Phase 0: startup model listing aggregates every backend (OpenAI unauthenticated
   + OpenCode Go Bearer)
4. Phase A: backend=opencode-go + deepseek-v4.1-flash -> POST /v1/chat/completions
   (Bearer auth), assert session header == cafe session id, streamed text
5. Phase B: backend=opencode-go + grok-4.6 -> POST /v1/responses (Bearer auth)
6. Phase C: backend=opencode-go + minimax-m3 -> POST /v1/messages (x-api-key auth
   + anthropic-version)
7. Phase D: backend=opencode-go with no model -> that backend's default model
8. Phase E: model set with no backend -> process-default OpenAI backend, and no
   x-opencode-session header

Usage:
    cargo build --release
    uv run tests/opencode-go-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")
LLM_BIN = os.path.join(RELEASE_DIR, "cafe-llm")
AGENT_BIN = os.path.join(RELEASE_DIR, "cafe-agent-runtime")
SERVER_BIN = os.path.join(RELEASE_DIR, "cafe-server")

MOCK_PORT = 49990
SERVER_PORT = 49989
TOKEN = "test-admin-token"
API_KEY = "test-opencode-go-key"

mock_requests = []
mock_lock = threading.Lock()

MODELS = [
    "deepseek-v4.1-flash",
    "grok-4.6",
    "minimax-m3",
    "glm-5.3",
]


class MockOpenCodeGoHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _record(self, method, body):
        with mock_lock:
            mock_requests.append({
                "method": method,
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
                "x_api_key": self.headers.get("x-api-key"),
                "anthropic_version": self.headers.get("anthropic-version"),
                "session": self.headers.get("x-opencode-session"),
                "user_agent": self.headers.get("User-Agent"),
                "body": body,
            })

    def do_GET(self):
        self._record("GET", None)
        if self.path == "/v1/models":
            payload = {
                "object": "list",
                "data": [{"id": m, "object": "model", "owned_by": "opencode"} for m in MODELS],
            }
            body = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()

    def _sse_start(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length).decode()
        body = json.loads(raw) if raw else {}
        self._record("POST", body)

        if self.path == "/v1/chat/completions":
            self._sse_start()
            for word in "chat-ok-marker".split():
                chunk = {"id": "mock-0", "object": "chat.completion.chunk",
                         "choices": [{"delta": {"content": word + " "}, "index": 0}]}
                self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                self.wfile.flush()
            final = {"id": "mock-0", "object": "chat.completion.chunk",
                     "choices": [{"delta": {}, "finish_reason": "stop", "index": 0}]}
            self.wfile.write(f"data: {json.dumps(final)}\n\n".encode())
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        elif self.path == "/v1/responses":
            self._sse_start()
            for word in "responses-ok-marker".split():
                event = {"type": "response.output_text.delta", "delta": word + " "}
                self.wfile.write(f"event: response.output_text.delta\ndata: {json.dumps(event)}\n\n".encode())
                self.wfile.flush()
            done = {"type": "response.completed", "response": {"status": "completed"}}
            self.wfile.write(f"event: response.completed\ndata: {json.dumps(done)}\n\n".encode())
            self.wfile.flush()
        elif self.path == "/v1/messages":
            self._sse_start()
            start = {"type": "content_block_start", "index": 0,
                     "content_block": {"type": "text", "text": ""}}
            self.wfile.write(f"event: content_block_start\ndata: {json.dumps(start)}\n\n".encode())
            for word in "messages-ok-marker".split():
                delta = {"type": "content_block_delta", "index": 0,
                         "delta": {"type": "text_delta", "text": word + " "}}
                self.wfile.write(f"event: content_block_delta\ndata: {json.dumps(delta)}\n\n".encode())
                self.wfile.flush()
            stop = {"type": "message_stop"}
            self.wfile.write(f"event: message_stop\ndata: {json.dumps(stop)}\n\n".encode())
            self.wfile.flush()
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()

    def log_message(self, format, *args):
        pass


def run_mock_server():
    server = ThreadingHTTPServer(("0.0.0.0", MOCK_PORT), MockOpenCodeGoHandler)
    server.daemon_threads = True
    server.serve_forever()


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
    asst = [c for c in chunks if c.get("annotations", {}).get("chat.role") == "assistant"]
    return "".join(c.get("content", "") or "" for c in asst)


def last_post():
    with mock_lock:
        posts = [r for r in mock_requests if r["method"] == "POST"]
    assert posts, "No POST requests reached the mock OpenCode Go gateway"
    return posts[-1]


def wait_for_models_listings(timeout=20):
    """Wait for cafe-llm's startup model listing.

    The router aggregates models from every configured backend, so the mock
    receives a GET /v1/models from the process-default OpenAI backend (no auth)
    and one from the OpenCode Go backend (Bearer). Return all recorded listings.
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        with mock_lock:
            lists = [r for r in mock_requests if r["method"] == "GET" and r["path"] == "/v1/models"]
        if len(lists) >= 2:
            return lists
        time.sleep(0.25)
    raise AssertionError(
        f"cafe-llm never listed models from all backends (saw {len(lists)} GET /v1/models)"
    )


def assert_common(req, session_id):
    assert req["session"] == session_id, (
        f"x-opencode-session must equal the cafe session id {session_id!r}, got {req['session']!r}"
    )
    ua = req["user_agent"] or ""
    assert ua.lower().startswith("cafe-llm/"), (
        f"OpenCode Go requires a client-identifying User-Agent, got {ua!r}"
    )


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-store", STORE_BIN), ("cafe-llm", LLM_BIN),
                       ("cafe-agent-runtime", AGENT_BIN), ("cafe-server", SERVER_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        agents_dir = os.path.join(tmpdir, "agents")
        os.mkdir(agents_dir)
        with open(os.path.join(agents_dir, "opencode-go-e2e.toml"), "w") as f:
            f.write(
                'name = "opencode-go-e2e"\n'
                'description = "opencode go fixture"\n'
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
        # Process default is a local OpenAI-compatible backend (the mock); the
        # OpenCode Go provider is selected per session via config.llm.backend.
        env["LLM_BACKEND"] = "openai"
        env["OPENAI_URL"] = f"http://localhost:{MOCK_PORT}"
        env["OPENAI_MODEL"] = "local-mock"
        env["OLLAMA_URL"] = f"http://localhost:{MOCK_PORT}"
        env["OLLAMA_MODEL"] = "local-ollama"
        env["OPENCODE_GO_URL"] = f"http://localhost:{MOCK_PORT}"
        env["OPENCODE_API_KEY"] = API_KEY
        env["OPENCODE_GO_MODEL"] = "deepseek-v4.1-flash"
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
            r = subprocess.run([CLI, "--bus", bus_socket, "create-session", "--agent", "opencode-go-e2e"],
                               capture_output=True, text=True)
            assert r.returncode == 0, r.stderr
            session_id = r.stdout.strip()
            print(f"  session={session_id}", file=sys.stderr)

            # ── Phase 0: startup model listing across backends ──
            print("=== Phase 0: GET /v1/models (all backends) ===", file=sys.stderr)
            listings = wait_for_models_listings()
            auths = [l["authorization"] for l in listings]
            assert f"Bearer {API_KEY}" in auths, (
                f"OpenCode Go listing must use Bearer auth, got {auths}"
            )
            assert None in auths, (
                f"process-default OpenAI backend must list without auth, got {auths}"
            )
            go_listing = next(l for l in listings if l["authorization"] == f"Bearer {API_KEY}")
            go_ua = (go_listing["user_agent"] or "").lower()
            assert go_ua.startswith("cafe-llm/"), (
                f"OpenCode Go listing must identify as cafe-llm, got {go_listing['user_agent']!r}"
            )
            print("  ✅ model listing from all backends confirmed", file=sys.stderr)

            # ── Phase A: select OpenCode Go per session, chat/completions ──
            print("=== Phase A: backend=opencode-go + deepseek-v4.1-flash -> /v1/chat/completions ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.backend", "opencode-go"),
                ("config.llm.model", "deepseek-v4.1-flash"),
            ])
            time.sleep(1)

            text = chat(bus_socket, session_id, "chat-phase-alpha")
            req = last_post()
            assert req["path"] == "/v1/chat/completions", f"expected chat/completions, got {req['path']}"
            assert req["authorization"] == f"Bearer {API_KEY}", (
                f"chat/completions must use Bearer auth, got {req['authorization']!r}"
            )
            assert req["x_api_key"] is None, "chat/completions must not send x-api-key"
            assert_common(req, session_id)
            assert req["body"]["model"] == "deepseek-v4.1-flash", req["body"]["model"]
            assert any("chat-phase-alpha" in (m.get("content") or "") for m in req["body"]["messages"]), (
                "user text must reach the backend"
            )
            assert "chat-ok-marker" in text, f"assistant marker missing from stream: {text!r}"
            print("  ✅ per-session backend=opencode-go routed to chat/completions", file=sys.stderr)

            # ── Phase B: Responses (Grok) ──
            print("=== Phase B: grok-4.6 -> /v1/responses ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.backend", "opencode-go"),
                ("config.llm.model", "grok-4.6"),
            ])
            time.sleep(1)

            text = chat(bus_socket, session_id, "responses-phase-bravo")
            req = last_post()
            assert req["path"] == "/v1/responses", f"expected /v1/responses, got {req['path']}"
            assert req["authorization"] == f"Bearer {API_KEY}", (
                f"/v1/responses must use Bearer auth, got {req['authorization']!r}"
            )
            assert_common(req, session_id)
            assert req["body"]["model"] == "grok-4.6", req["body"]["model"]
            assert isinstance(req["body"]["input"], list), "Responses request needs an input list"
            assert "responses-ok-marker" in text, f"assistant marker missing from stream: {text!r}"
            print("  ✅ /v1/responses route + auth + session header confirmed", file=sys.stderr)

            # ── Phase C: Anthropic Messages (MiniMax) ──
            print("=== Phase C: minimax-m3 -> /v1/messages ===", file=sys.stderr)
            publish_config(bus_socket, session_id, [
                ("config.llm.backend", "opencode-go"),
                ("config.llm.model", "minimax-m3"),
            ])
            time.sleep(1)

            text = chat(bus_socket, session_id, "messages-phase-charlie")
            req = last_post()
            assert req["path"] == "/v1/messages", f"expected /v1/messages, got {req['path']}"
            assert req["x_api_key"] == API_KEY, (
                f"/v1/messages must use x-api-key auth, got {req['x_api_key']!r}"
            )
            assert req["anthropic_version"], "/v1/messages requires an anthropic-version header"
            assert_common(req, session_id)
            assert req["body"]["model"] == "minimax-m3", req["body"]["model"]
            assert req["body"].get("max_tokens"), "Messages request requires max_tokens"
            assert "messages-ok-marker" in text, f"assistant marker missing from stream: {text!r}"
            print("  ✅ /v1/messages route + auth + session header confirmed", file=sys.stderr)

            # ── Phase D: backend only -> that backend's default model ──
            print("=== Phase D: backend=opencode-go (no model) -> Go default ===", file=sys.stderr)
            r = subprocess.run([CLI, "--bus", bus_socket, "create-session", "--agent", "opencode-go-e2e"],
                               capture_output=True, text=True)
            assert r.returncode == 0, r.stderr
            session_d = r.stdout.strip()
            publish_config(bus_socket, session_d, [
                ("config.llm.backend", "opencode-go"),
            ])
            time.sleep(1)

            text = chat(bus_socket, session_d, "backend-only-phase-delta")
            req = last_post()
            assert req["path"] == "/v1/chat/completions", (
                f"Go default model must use chat/completions, got {req['path']}"
            )
            assert_common(req, session_d)
            assert req["body"]["model"] == "deepseek-v4.1-flash", (
                f"backend-only session must fall back to OPENCODE_GO_MODEL, got {req['body']['model']!r}"
            )
            assert "chat-ok-marker" in text, f"assistant marker missing from stream: {text!r}"
            print("  ✅ backend-only session uses that backend's default model", file=sys.stderr)

            # ── Phase E: model only -> process default backend (no Go session header) ──
            print("=== Phase E: model=local-mock (no backend) -> default OpenAI backend ===", file=sys.stderr)
            r = subprocess.run([CLI, "--bus", bus_socket, "create-session", "--agent", "opencode-go-e2e"],
                               capture_output=True, text=True)
            assert r.returncode == 0, r.stderr
            session_e = r.stdout.strip()
            publish_config(bus_socket, session_e, [
                ("config.llm.model", "local-mock"),
            ])
            time.sleep(1)

            text = chat(bus_socket, session_e, "default-backend-phase-echo")
            req = last_post()
            assert req["path"] == "/v1/chat/completions", f"expected chat/completions, got {req['path']}"
            assert req["session"] is None, (
                f"OpenAI backend must not send x-opencode-session, got {req['session']!r}"
            )
            assert req["body"]["model"] == "local-mock", req["body"]["model"]
            assert "chat-ok-marker" in text, f"assistant marker missing from stream: {text!r}"
            print("  ✅ session without backend uses the process default backend", file=sys.stderr)

            print("\n=== ALL OPENCODE GO TESTS PASSED ===", file=sys.stderr)

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
