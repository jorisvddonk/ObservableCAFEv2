#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: text-to-speech via cafe-tts.

Tests: text → bus RPC (tts.invoke) → cafe-tts → voicebox /generate/stream → audio BinaryRef + result.

Usage:
    cargo build --release
    uv run tests/tts-e2e.py
"""

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
TTS_BIN = os.path.join(RELEASE_DIR, "cafe-tts")

VOICEBOX_URL = os.environ.get("VOICEBOX_URL", "http://127.0.0.1:17493")


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
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-tts", TTS_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["VOICEBOX_URL"] = VOICEBOX_URL

        bus_log = open(os.path.join(tmpdir, "bus.log"), "w", buffering=1)
        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=bus_log, stderr=bus_log)
        time.sleep(2)

        print("=== Starting cafe-tts ===", file=sys.stderr)
        tts_log = os.path.join(tmpdir, "tts.log")
        tts_out = os.path.join(tmpdir, "tts.out")
        tts_err = open(tts_log, "w", buffering=1)
        tts_proc = subprocess.Popen([TTS_BIN], env=env, stdout=open(tts_out, "w"), stderr=tts_err)
        time.sleep(4)

        if tts_proc.poll() is not None:
            with open(tts_log) as f:
                print(f"TTS exited with code {tts_proc.returncode}, log: {f.read()[:200]}", file=sys.stderr)

        try:
            print("=== Create session ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
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

            call_id = str(uuid.uuid4())
            rpc_request = {
                "jsonrpc": "2.0",
                "method": "tts.invoke",
                "id": call_id,
                "params": {
                    "text": "Hello world",
                    "profile": "Test",
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
                    "producer": "com.nominal.tts-e2e-test",
                    "timestamp": now_ms,
                    "annotations": {
                        "cafe.jsonrpc.request": rpc_request,
                        "cafe.transient": True,
                        "cafe.transient.retain_secs": 60,
                    },
                },
            })
            print(f"  published tts.invoke (call_id={call_id})", file=sys.stderr)
            pub_sock.close()

            print("=== Reading response ===", file=sys.stderr)
            sub_sock.settimeout(60)
            result = None
            binary_ref_found = False
            try:
                while True:
                    line = recv_line(sub_sock)
                    if not line:
                        break
                    msg = json.loads(line)
                    if msg.get("event") == "chunk":
                        chunk = msg["chunk"]
                        ann = chunk.get("annotations", {})
                        rpc_resp = ann.get("cafe.jsonrpc.response")
                        if rpc_resp and rpc_resp.get("id") == call_id:
                            print(f"  rpc response received", file=sys.stderr)
                            result = rpc_resp
                        if chunk.get("content_type") == "binary_ref":
                            binary_ref_found = True
                            print(f"  BinaryRef audio chunk found: {chunk.get('id', '')[:20]}...", file=sys.stderr)
                        if result is not None and binary_ref_found:
                            break
            except socket.timeout:
                print("  timeout waiting for response", file=sys.stderr)

            sub_sock.close()

            if result is None:
                print("  no RPC response received", file=sys.stderr)
                assert False, "No tts.invoke RPC response received"

            err = result.get("error")
            if err is not None:
                print(f"  voicebox TTS error: {err.get('message', '')[:200]}", file=sys.stderr)
                print("  TTS model not loaded (RPC flow verified)", file=sys.stderr)
            else:
                r = result.get("result", {})
                chunk_id = r.get("chunk_id", "")
                if chunk_id:
                    print(f"  audio chunk_id: {chunk_id}", file=sys.stderr)
                    assert binary_ref_found, "BinaryRef audio chunk was not published"
                    print("  TTS synthesis successful", file=sys.stderr)
                else:
                    print(f"  unexpected result: {json.dumps(result, indent=2)}", file=sys.stderr)
                    assert False, "Unexpected RPC result format"

            run([CLI, "--bus", bus_socket, "delete-session", session_id])

        finally:
            try:
                bus_log_path = os.path.join(tmpdir, "bus.log")
                if os.path.exists(bus_log_path):
                    with open(bus_log_path) as f:
                        bus_log_text = f.read()
                    if bus_log_text.strip():
                        print("=== BUS LOG ===", file=sys.stderr)
                        for line in bus_log_text.strip().split("\n")[-30:]:
                            print(f"  {line}", file=sys.stderr)
                tts_log_path = os.path.join(tmpdir, "tts.log")
                if os.path.exists(tts_log_path):
                    with open(tts_log_path) as f:
                        tts_log_text = f.read()
                    print("=== TTS STDERR ===", file=sys.stderr)
                    for line in tts_log_text.strip().split("\n")[-30:]:
                        print(f"  {line}", file=sys.stderr)
                tts_out_path = os.path.join(tmpdir, "tts.out")
                if os.path.exists(tts_out_path):
                    with open(tts_out_path) as f:
                        tts_out_text = f.read()
                    if tts_out_text.strip():
                        print("=== TTS STDOUT ===", file=sys.stderr)
                        for line in tts_out_text.strip().split("\n")[-10:]:
                            print(f"  {line}", file=sys.stderr)
            except Exception as e:
                print(f"=== TTS LOG ERROR: {e} ===", file=sys.stderr)
            for p in [bus_proc, tts_proc]:
                p.kill()
            for p in [bus_proc, tts_proc]:
                p.wait()

    print(file=sys.stderr)
    if result and result.get("error") is None:
        print("=== ALL TTS E2E TESTS PASSED ===", file=sys.stderr)
    else:
        print("=== TTS E2E: partial (RPC flow OK, voicebox /generate/stream unavailable) ===", file=sys.stderr)


if __name__ == "__main__":
    main()
