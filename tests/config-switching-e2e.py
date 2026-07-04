#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: null config chunks switch model and system prompt.

Flow:
1. Start cafe-bus + mock LLM + cafe-llm + cafe-store + cafe-agent-runtime + cafe-server
2. Create session with default agent
3. Publish config chunk: model=gemma3:1b, system_prompt="You are a cat"
4. Send chat message — verify mock LLM received gemma3:1b + cat prompt
5. Publish config chunk: model=llama3.2:3b, system_prompt="You are a dog"
6. Send chat message — verify mock LLM received llama3.2:3b + dog prompt

Usage:
    cargo build --release
    uv run tests/config-switching-e2e.py
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

MOCK_PORT = 49995
SERVER_PORT = 49994
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


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def start_proc(cmd, env, logfile):
    return subprocess.Popen(cmd, env=env, stdout=open(logfile, "w"), stderr=subprocess.STDOUT)


def cli(socket_path, *args):
    r = run([CLI, "--bus", socket_path, "--server", "http://localhost:49994",
             "--token", TOKEN, *args])
    assert r.returncode == 0, r.stderr
    return r.stdout.strip()


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-store", STORE_BIN), ("cafe-llm", LLM_BIN),
                        ("cafe-agent-runtime", AGENT_BIN), ("cafe-server", SERVER_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["CAFE_DB_PATH"] = db_path
        env["CAFE_ADMIN_TOKEN"] = TOKEN
        env["PORT"] = str(SERVER_PORT)
        env["LLM_BACKEND"] = "openai"
        env["OPENAI_URL"] = f"http://localhost:{MOCK_PORT}/v1"
        env["OPENAI_MODEL"] = "gemma3:1b"

        # Start mock LLM server thread
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
            # Create session with default agent
            print("=== Create session ===", file=sys.stderr)
            r = subprocess.run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"],
                               capture_output=True, text=True)
            assert r.returncode == 0
            session_id = r.stdout.strip()
            print(f"  session={session_id}", file=sys.stderr)

            # ── Config 1: gemma3:1b, cat system prompt ──
            print("=== Config 1: gemma3:1b, cat ===", file=sys.stderr)
            mock_requests.clear()
            # Publish config chunk via cafe-cli --null --annotation config.type=runtime etc.
            r = subprocess.run(
                [CLI, "--bus", bus_socket, "publish", session_id, "--null",
                 "--annotation", "config.type=runtime",
                 "--annotation", "config.llm.model=gemma3:1b",
                 "--annotation", "config.llm.system_prompt=You are a cat. Respond like a cat."],
                capture_output=True, text=True,
            )
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            print(f"  published config chunk (model=gemma3:1b, cat)", file=sys.stderr)
            time.sleep(1)

            # Chat
            c = cli(bus_socket, "chat", session_id, "hello, what are you?", "--timeout-secs", "60")
            chunks = [json.loads(l) for l in c.split("\n") if l.strip()]
            print(f"  {len(chunks)} SSE chunks", file=sys.stderr)

            # Verify mock received gemma3 and cat prompt
            with mock_lock:
                reqs = list(mock_requests)
            assert len(reqs) >= 1, "No LLM requests logged"
            last = reqs[-1]
            assert "gemma3" in last["model"], f"Expected gemma3, got {last['model']}"
            assert "cat" in last["system_prompt"].lower(), f"Expected cat prompt, got {last['system_prompt']}"
            print(f"  ✅ gemma3:1b + cat prompt confirmed", file=sys.stderr)

            # ── Config 2: llama3.2:3b, dog system prompt ──
            print("=== Config 2: llama3.2, dog ===", file=sys.stderr)
            r = subprocess.run(
                [CLI, "--bus", bus_socket, "publish", session_id, "--null",
                 "--annotation", "config.type=runtime",
                 "--annotation", "config.llm.model=llama3.2:3b",
                 "--annotation", "config.llm.system_prompt=You are a dog. Respond like a dog. Woof!"],
                capture_output=True, text=True,
            )
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            print(f"  published config chunk (model=llama3.2:3b, dog)", file=sys.stderr)
            time.sleep(1)

            c = cli(bus_socket, "chat", session_id, "what's your name?", "--timeout-secs", "60")
            chunks = [json.loads(l) for l in c.split("\n") if l.strip()]
            print(f"  {len(chunks)} SSE chunks", file=sys.stderr)

            with mock_lock:
                reqs = list(mock_requests)
            last = reqs[-1]
            assert "llama3.2" in last["model"], f"Expected llama3.2, got {last['model']}"
            assert "dog" in last["system_prompt"].lower(), f"Expected dog prompt, got {last['system_prompt']}"
            print(f"  ✅ llama3.2:3b + dog prompt confirmed", file=sys.stderr)

            print("\n=== ALL CONFIG SWITCHING TESTS PASSED ===", file=sys.stderr)

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
