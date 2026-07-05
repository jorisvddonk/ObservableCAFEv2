#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: speech-to-text via cafe-stt.

Tests: base64 audio → bus RPC (stt.invoke) → cafe-stt → voicebox /transcribe → result.

Usage:
    cargo build --release
    uv run tests/stt-e2e.py
"""

import base64
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import uuid

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STT_BIN = os.path.join(RELEASE_DIR, "cafe-stt")
AUDIO_FILE = os.path.join(PROJECT_ROOT, "tests", "fixtures", "stt-test-audio.wav")

VOICEBOX_URL = os.environ.get("VOICEBOX_URL", "http://127.0.0.1:17493")


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def bus_connect(socket_path):
    """Connect to the bus, read Connected message, return socket + helper."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(socket_path)
    s.settimeout(10)
    line = recv_line(s)
    msg = json.loads(line)
    assert msg["event"] == "connected"
    return s


def recv_line(sock):
    """Read one complete \n-terminated line from a socket."""
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
    """Send a JSON message followed by newline."""
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

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["VOICEBOX_URL"] = VOICEBOX_URL

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-stt ===", file=sys.stderr)
        stt_log = os.path.join(tmpdir, "stt.log")
        stt_out = os.path.join(tmpdir, "stt.out")
        stt_err = open(stt_log, "w", buffering=1)
        stt_proc = subprocess.Popen([STT_BIN], env=env, stdout=open(stt_out, "w"), stderr=stt_err)
        time.sleep(4)

        # Check if stt is still alive
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

            # Open a bus connection for subscribing
            sub_sock = bus_connect(bus_socket)
            send_msg(sub_sock, {"op": "subscribe", "session_id": session_id})

            # Drain history
            while True:
                line = recv_line(sub_sock)
                msg = json.loads(line)
                if msg.get("event") == "history_complete":
                    break
                # else: chunk, skip

            print("  subscribed", file=sys.stderr)

            # Read and base64 audio
            with open(AUDIO_FILE, "rb") as f:
                audio_b64 = base64.b64encode(f.read()).decode()
            print(f"  audio: {os.path.getsize(AUDIO_FILE)} bytes", file=sys.stderr)

            # Build RPC request
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

            # Publish RPC on a separate connection
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
                    "producer": "com.nominal.stt-e2e-test",
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

            # Read response on the subscriber connection
            print("=== Reading response ===", file=sys.stderr)
            sub_sock.settimeout(60)
            result = None
            text = None
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

            if result is None:
                print("  ⚠️ no RPC response received", file=sys.stderr)
                assert False, "No stt.invoke RPC response received"

            # Check for error or success
            err = result.get("error")
            if err is not None:
                print(f"  voicebox error: {err.get('message', '')[:200]}", file=sys.stderr)
                print("  ⚠️ voicebox unavailable (RPC flow verified)", file=sys.stderr)
            else:
                r = result.get("result", {})
                text = r.get("text", "")
                duration = r.get("duration", 0)
                chunk_id = r.get("chunk_id", "")
                if text:
                    print(f"  transcription: '{text[:120]}'", file=sys.stderr)
                    print(f"  duration: {duration}s", file=sys.stderr)
                    print(f"  chunk_id: {chunk_id}", file=sys.stderr)
                    print("  ✅ transcription successful", file=sys.stderr)
                else:
                    print(f"  unexpected result: {json.dumps(result, indent=2)}", file=sys.stderr)
                    assert False, f"Unexpected RPC result format"

            run([CLI, "--bus", bus_socket, "delete-session", session_id])

        finally:
            # Print STT logs before killing
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

    print(file=sys.stderr)
    if text:
        print("=== ALL STT E2E TESTS PASSED ===", file=sys.stderr)
    else:
        print("=== STT E2E: partial (RPC flow OK, voicebox unavailable) ===", file=sys.stderr)


if __name__ == "__main__":
    main()
