#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: text-to-speech + binary upload via cafe-tts.

Tests: bus RPC (tts.invoke) → cafe-tts → voicebox /generate/stream →
       BinaryRef → cafe-binary-store → write credentials →
       HTTP upload → read credentials → upload completion.

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
BINARY_STORE_BIN = os.path.join(RELEASE_DIR, "cafe-binary-store")

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
    binaries = {
        "cafe-bus": BUS_BIN,
        "cafe-tts": TTS_BIN,
        "cafe-cli": CLI,
        "cafe-binary-store": BINARY_STORE_BIN,
    }
    for name, path in binaries.items():
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        binary_data_dir = os.path.join(tmpdir, "binary-store-data")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["VOICEBOX_URL"] = VOICEBOX_URL

        procs = {}

        bus_log = open(os.path.join(tmpdir, "bus.log"), "w", buffering=1)
        print("=== Starting cafe-bus ===", file=sys.stderr)
        procs["cafe-bus"] = subprocess.Popen([BUS_BIN], env=env, stdout=bus_log, stderr=bus_log)
        time.sleep(2)

        print("=== Starting cafe-binary-store ===", file=sys.stderr)
        bs_log = open(os.path.join(tmpdir, "binary-store.log"), "w", buffering=1)
        procs["cafe-binary-store"] = subprocess.Popen(
            [BINARY_STORE_BIN,
             "--bus-socket", bus_socket,
             "--port", "4003",
             "--data-dir", binary_data_dir,
             "--public-host", "localhost"],
            env=env, stdout=bs_log, stderr=subprocess.STDOUT,
        )
        time.sleep(2)

        print("=== Starting cafe-tts ===", file=sys.stderr)
        tts_log = os.path.join(tmpdir, "tts.log")
        tts_out = os.path.join(tmpdir, "tts.out")
        tts_err = open(tts_log, "w", buffering=1)
        procs["cafe-tts"] = subprocess.Popen([TTS_BIN], env=env, stdout=open(tts_out, "w"), stderr=tts_err)
        time.sleep(4)

        for name, p in list(procs.items()):
            if p.poll() is not None:
                print(f"  {name} exited with code {p.returncode}", file=sys.stderr)
                for n, lp in procs.items():
                    log_path = os.path.join(tmpdir, f"{n}.log")
                    if os.path.exists(log_path):
                        with open(log_path) as f:
                            print(f"  --- {n} log ---", file=sys.stderr)
                            print(f.read()[-1000:], file=sys.stderr)
                assert False, f"Service {name} failed to start"

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
            binary_ref_id = None
            audio_byte_size = None
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
                            result = rpc_resp
                            print(f"  rpc response received", file=sys.stderr)
                        if chunk.get("content_type") == "binary_ref":
                            binary_ref_id = chunk["id"]
                            audio_byte_size = ann.get("cafe.binary.byte_size")
                            print(f"  BinaryRef audio chunk: {binary_ref_id[:20]}... ({audio_byte_size} bytes)", file=sys.stderr)
                        if result is not None and binary_ref_id is not None:
                            break
            except socket.timeout:
                print("  timeout waiting for response", file=sys.stderr)
                assert False, "Timeout waiting for tts.invoke RPC response or BinaryRef chunk"

            assert result is not None, "No tts.invoke RPC response received"
            assert binary_ref_id is not None, "No BinaryRef audio chunk received"
            assert audio_byte_size is not None, "BinaryRef missing cafe.binary.byte_size"
            assert audio_byte_size > 0, "BinaryRef byte_size is zero"

            err = result.get("error")
            assert err is None, f"TTS synthesis failed: {err.get('message', '')}"

            r = result.get("result", {})
            chunk_id = r.get("chunk_id", "")
            assert chunk_id, f"Missing chunk_id in TTS result: {json.dumps(result, indent=2)}"
            print(f"  TTS synthesis successful (chunk_id={chunk_id})", file=sys.stderr)

            # Verify binary upload lifecycle via broadcast mutations.
            # Phase 1 (write creds) goes via publish_direct — not visible here.
            # Phases 2 (read creds) and 3 (completion) are broadcast.
            print("=== Verifying binary upload lifecycle ===", file=sys.stderr)
            read_creds_found = False
            completed_found = False

            sub_sock.settimeout(60)
            try:
                while True:
                    line = recv_line(sub_sock)
                    if not line:
                        break
                    msg = json.loads(line)
                    if msg.get("event") == "chunk":
                        chunk = msg["chunk"]
                        ann = chunk.get("annotations", {})
                        target = ann.get("cafe.mutates.target_id")
                        if target == binary_ref_id:
                            if ann.get("cafe.binary.read_url"):
                                read_creds_found = True
                                print(f"  phase 2/3: read credentials received", file=sys.stderr)
                            if ann.get("cafe.binary.completed"):
                                completed_found = True
                                print(f"  phase 3/3: upload completed", file=sys.stderr)
                                break
            except socket.timeout:
                print("  timeout waiting for upload lifecycle", file=sys.stderr)

            assert read_creds_found, "Binary upload failed: no read credentials mutation"
            assert completed_found, "Binary upload failed: no upload completion mutation"
            print("  binary upload lifecycle verified", file=sys.stderr)

            run([CLI, "--bus", bus_socket, "delete-session", session_id])

        finally:
            for name in ["cafe-bus", "cafe-tts", "cafe-binary-store"]:
                log_path = os.path.join(tmpdir, f"{name}.log")
                if os.path.exists(log_path):
                    try:
                        with open(log_path) as f:
                            content = f.read().strip()
                        if content:
                            print(f"=== {name} LOG ===", file=sys.stderr)
                            for line in content.split("\n")[-30:]:
                                print(f"  {line}", file=sys.stderr)
                    except Exception:
                        pass
            tts_log_path = os.path.join(tmpdir, "tts.log")
            if os.path.exists(tts_log_path):
                try:
                    with open(tts_log_path) as f:
                        content = f.read().strip()
                    if content:
                        print("=== TTS STDERR ===", file=sys.stderr)
                        for line in content.split("\n")[-20:]:
                            print(f"  {line}", file=sys.stderr)
                except Exception:
                    pass
            for p in procs.values():
                p.kill()
            for p in procs.values():
                p.wait()

    print(file=sys.stderr)
    print("=== ALL TTS E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
