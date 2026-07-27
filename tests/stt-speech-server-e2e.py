#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: STT via speech-server backend.

Same flow as stt-e2e.py but uses STT_BACKEND=speech-server and a mock
HTTP server standing in for the real speech-server/proxy.

Usage:
    cargo build --release
    uv run tests/stt-speech-server-e2e.py
"""

import base64
import http.server
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STT_BIN = os.path.join(RELEASE_DIR, "cafe-stt")
AUDIO_FILE = os.path.join(PROJECT_ROOT, "tests", "fixtures", "stt-test-audio.wav")

MOCK_PORT = 18790


class MockSpeechServerHandler(http.server.BaseHTTPRequestHandler):
    """Minimal mock that replies to POST /transcribe with JSON response."""

    def do_POST(self):
        if self.path.startswith("/transcribe"):
            content_len = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_len)
            text = "hello world"
            resp = json.dumps({
                "text": text,
                "duration": 1.5,
                "language": "en",
                "confidence": 0.95,
            })
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(resp)))
            self.end_headers()
            self.wfile.write(resp.encode())
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, fmt, *args):
        pass


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def bus_connect(socket_path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(socket_path)
    s.settimeout(10)
    send_msg(s, {"op": "ping"})
    line = recv_line(s)
    msg = json.loads(line)
    assert msg["event"] == "connected" or msg["event"] == "codec_set"
    return s


def recv_line(sock):
    data = b""
    while True:
        b = sock.recv(1)
        if not b:
            break
        data += b
        if b == b"\n":
            break
    return data


def send_msg(sock, msg):
    sock.sendall((json.dumps(msg) + "\n").encode())


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-stt", STT_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    if not os.path.exists(AUDIO_FILE):
        print(f"Missing audio fixture: {AUDIO_FILE}", file=sys.stderr)
        print("Generate one with: say -o tests/fixtures/stt-test-audio.wav '<text>'", file=sys.stderr)
        sys.exit(1)

    # Start mock speech-server
    mock_server = http.server.HTTPServer(("127.0.0.1", MOCK_PORT), MockSpeechServerHandler)
    mock_thread = threading.Thread(target=mock_server.serve_forever, daemon=True)
    mock_thread.start()
    print(f"  mock speech-server started on :{MOCK_PORT}", file=sys.stderr)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["STT_BACKEND"] = "speech-server"
        env["SPEECH_SERVER_URL"] = f"http://127.0.0.1:{MOCK_PORT}"

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-stt (speech-server backend) ===", file=sys.stderr)
        stt_log = os.path.join(tmpdir, "stt.log")
        stt_out = os.path.join(tmpdir, "stt.out")
        stt_err = open(stt_log, "w", buffering=1)
        stt_proc = subprocess.Popen([STT_BIN], env=env, stdout=open(stt_out, "w"), stderr=stt_err)
        time.sleep(4)

        if stt_proc.poll() is not None:
            with open(stt_log) as f:
                print(f"STT exited with code {stt_proc.returncode}, log: {f.read()[:200]}", file=sys.stderr)

        try:
            print("=== Create session ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "stt"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            session_id = r.stdout.strip()
            print(f"  session={session_id}", file=sys.stderr)
            time.sleep(1)

            sub_sock = bus_connect(bus_socket)
            send_msg(sub_sock, {"op": "subscribe", "session_id": session_id})

            while True:
                line = recv_line(sub_sock)
                msg = json.loads(line)
                if msg.get("event") == "history_complete":
                    break

            print("  subscribed", file=sys.stderr)

            with open(AUDIO_FILE, "rb") as f:
                audio_b64 = base64.b64encode(f.read()).decode()
            print(f"  audio: {os.path.getsize(AUDIO_FILE)} bytes", file=sys.stderr)

            call_id = str(uuid.uuid4())
            rpc_request = {
                "jsonrpc": "2.0",
                "method": "stt.invoke",
                "id": call_id,
                "params": {
                    "audio": audio_b64,
                    "mime_type": "audio/wav",
                    "language": "en",
                },
            }

            pub_sock = bus_connect(bus_socket)
            now_ms = int(time.time() * 1000)
            send_msg(pub_sock, {
                "op": "publish",
                "session_id": session_id,
                "chunk": {
                    "id": str(uuid.uuid4()),
                    "content_type": "null",
                    "content": None,
                    "data": None,
                    "mime_type": None,
                    "producer": "com.nominal.stt-speech-server-e2e-test",
                    "timestamp": now_ms,
                    "annotations": {
                        "cafe.jsonrpc.request": rpc_request,
                        "cafe.transient": True,
                        "cafe.transient.retain_secs": 60,
                    },
                },
            })
            print(f"  published stt.invoke (call_id={call_id})", file=sys.stderr)
            pub_sock.close()

            print("=== Reading response ===", file=sys.stderr)
            sub_sock.settimeout(60)
            result = None
            try:
                while True:
                    line = recv_line(sub_sock)
                    if not line:
                        break
                    msg = json.loads(line)
                    if msg.get("event") == "chunk":
                        ann = msg["chunk"].get("annotations", {})
                        rpc_resp = ann.get("cafe.jsonrpc.response")
                        if rpc_resp and rpc_resp.get("id") == call_id:
                            print(f"  rpc response received", file=sys.stderr)
                            result = rpc_resp
                            break
            except socket.timeout:
                print("  timeout waiting for response", file=sys.stderr)

            sub_sock.close()

            assert result is not None, "No stt.invoke RPC response received"

            err = result.get("error")
            assert err is None, f"STT failed: {err.get('message', '')}"

            r = result.get("result", {})
            text = r.get("text", "")
            duration = r.get("duration", 0)
            chunk_id = r.get("chunk_id", "")
            assert text, f"Missing text in result: {json.dumps(result, indent=2)}"
            assert duration > 0, f"Missing or zero duration in result"
            assert chunk_id, f"Missing chunk_id in result"
            print(f"  transcription: '{text[:120]}'", file=sys.stderr)
            print(f"  duration: {duration}s", file=sys.stderr)
            print(f"  chunk_id: {chunk_id}", file=sys.stderr)

            run([CLI, "--bus", bus_socket, "delete-session", session_id])

        finally:
            try:
                stt_log_path = os.path.join(tmpdir, "stt.log")
                if os.path.exists(stt_log_path):
                    with open(stt_log_path) as f:
                        stt_log = f.read()
                    print("=== STT STDERR ===", file=sys.stderr)
                    for line in stt_log.strip().split("\n")[-30:]:
                        print(f"  {line}", file=sys.stderr)
                stt_out_path = os.path.join(tmpdir, "stt.out")
                if os.path.exists(stt_out_path):
                    with open(stt_out_path) as f:
                        stt_out = f.read()
                    if stt_out.strip():
                        print("=== STT STDOUT ===", file=sys.stderr)
                        for line in stt_out.strip().split("\n")[-10:]:
                            print(f"  {line}", file=sys.stderr)
            except Exception as e:
                print(f"=== STT LOG ERROR: {e} ===", file=sys.stderr)
            for p in [bus_proc, stt_proc]:
                p.kill()
            for p in [bus_proc, stt_proc]:
                p.wait()

    mock_server.shutdown()
    print(file=sys.stderr)
    print("=== ALL STT SPEECH-SERVER E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
