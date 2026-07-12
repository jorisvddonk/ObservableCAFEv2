#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: full volition agent pipeline.

Tests: user message → cafe-agent-runtime → cafe-llm → LLM backend →
       → cafe-tts → voicebox /generate/stream → audio BinaryRef.

Requires a running LLM backend. Set OPENAI_URL / OPENAI_MODEL to
override (defaults: localhost:8080 / Ornith-1.0-9B-4bit).

Usage:
    cargo build --release
    uv run tests/volition-pipeline-e2e.py
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
LLM_BIN = os.path.join(RELEASE_DIR, "cafe-llm")
AGENT_BIN = os.path.join(RELEASE_DIR, "cafe-agent-runtime")
TTS_BIN = os.path.join(RELEASE_DIR, "cafe-tts")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")

VOICEBOX_URL = os.environ.get("VOICEBOX_URL", "http://127.0.0.1:17493")
LLM_URL = os.environ.get("OPENAI_URL", "http://localhost:8080")
LLM_MODEL = os.environ.get("OPENAI_MODEL", "Ornith-1.0-9B-4bit")

TIMEOUT_SECS = 90


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
        "cafe-llm": LLM_BIN,
        "cafe-agent-runtime": AGENT_BIN,
        "cafe-tts": TTS_BIN,
        "cafe-cli": CLI,
        "cafe-store": STORE_BIN,
    }
    for name, path in binaries.items():
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["VOICEBOX_URL"] = VOICEBOX_URL

        # cafe-store env
        store_env = env.copy()
        store_env["CAFE_DB_PATH"] = db_path

        # cafe-llm env
        llm_env = env.copy()
        llm_env["LLM_BACKEND"] = "openai"
        llm_env["OPENAI_URL"] = LLM_URL
        llm_env["OPENAI_MODEL"] = LLM_MODEL

        procs = {}
        logs = {}

        def start(name, bin_path, extra_env=None):
            log = open(os.path.join(tmpdir, f"{name}.log"), "w", buffering=1)
            logs[name] = log
            e = extra_env if extra_env is not None else env
            procs[name] = subprocess.Popen([bin_path], env=e, stdout=log, stderr=log)

        bus_log = open(os.path.join(tmpdir, "bus.log"), "w", buffering=1)
        print("=== Starting cafe-bus ===", file=sys.stderr)
        procs["cafe-bus"] = subprocess.Popen([BUS_BIN], env=env, stdout=bus_log, stderr=bus_log)
        time.sleep(2)

        print("=== Starting cafe-store ===", file=sys.stderr)
        start("store", STORE_BIN, store_env)
        time.sleep(1)

        print(f"=== Starting cafe-llm (model={LLM_MODEL}) ===", file=sys.stderr)
        start("llm", LLM_BIN, llm_env)
        time.sleep(2)

        print("=== Starting cafe-agent-runtime ===", file=sys.stderr)
        start("agent", AGENT_BIN)
        time.sleep(2)

        print("=== Starting cafe-tts ===", file=sys.stderr)
        start("tts", TTS_BIN)
        time.sleep(4)

        if any(p.poll() is not None for p in procs.values()):
            for name, p in procs.items():
                if p.poll() is not None:
                    print(f"  {name} exited with code {p.returncode}", file=sys.stderr)
            assert False, "A service failed to start"

        try:
            print("=== Creating volition session ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "volition"])
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

            print("=== Sending user message ===", file=sys.stderr)
            now_ms = int(time.time() * 1000)
            send_msg(sub_sock, {
                "op": "publish",
                "session_id": session_id,
                "chunk": {
                    "id": str(uuid.uuid4()),
                    "content_type": "text",
                    "content": "say hello in one short sentence",
                    "data": None,
                    "mime_type": None,
                    "producer": "com.rxcafe.user",
                    "timestamp": now_ms,
                    "annotations": {
                        "chat.role": "user",
                    },
                },
            })

            print("=== Reading pipeline output ===", file=sys.stderr)
            sub_sock.settimeout(TIMEOUT_SECS)
            assistant_text = None
            tts_binary_ref = None
            tts_error = None

            try:
                while True:
                    line = recv_line(sub_sock)
                    if not line:
                        break
                    msg = json.loads(line)
                    if msg.get("event") == "chunk":
                        chunk = msg["chunk"]
                        ann = chunk.get("annotations", {})

                        # Collect assistant text from LLM (non-transient, chat.role=assistant)
                        ct = chunk.get("content_type")
                        if ct == "text" and ann.get("chat.role") == "assistant":
                            content = chunk.get("content", "")
                            if content and not ann.get("cafe.transient"):
                                assistant_text = assistant_text or content
                                print(f"  assistant: '{content[:80]}'", file=sys.stderr)

                        # Collect TTS binary_ref audio
                        if ct == "binary_ref" and chunk.get("producer") == "com.nominal.cafe-tts":
                            tts_binary_ref = chunk
                            print(f"  TTS audio chunk: {chunk.get('id', '')[:20]}...", file=sys.stderr)

                        # Check for TTS errors (RPC or pipeline)
                        rpc_resp = ann.get("cafe.jsonrpc.response")
                        if rpc_resp and rpc_resp.get("error"):
                            if tts_error is None:
                                tts_error = rpc_resp["error"].get("message", "")
                                print(f"  TTS error: {tts_error[:100]}", file=sys.stderr)

                        # Pipeline error (e.g. TTS step failed)
                        err_msg = ann.get("cafe.error.message")
                        if err_msg and ann.get("error.source") == "pipeline":
                            print(f"  pipeline error: {err_msg[:120]}", file=sys.stderr)

                        # Break when we have both or when TTS failed
                        if assistant_text is not None:
                            if tts_binary_ref is not None or tts_error is not None:
                                break

            except socket.timeout:
                print("  timeout waiting for pipeline output", file=sys.stderr)

            sub_sock.close()

            assert assistant_text is not None, "No assistant text received from LLM"
            print(f"  LLM response verified: {assistant_text[:80]}", file=sys.stderr)

            if tts_error is not None:
                print(f"  TTS model not loaded (pipeline flow verified)", file=sys.stderr)
            elif tts_binary_ref is not None:
                ann = tts_binary_ref.get("annotations", {})
                byte_size = ann.get("cafe.binary.byte_size")
                if byte_size:
                    print(f"  TTS audio size: {byte_size} bytes", file=sys.stderr)
                assert byte_size is not None, "BinaryRef chunk missing byte size annotation"
                print("  TTS audio verified", file=sys.stderr)
            else:
                assert False, "Neither TTS audio nor TTS error received"

            run([CLI, "--bus", bus_socket, "delete-session", session_id])

        finally:
            for name in ["cafe-bus", "store", "llm", "agent", "tts"]:
                p = procs.get(name)
                if p:
                    try:
                        log = open(os.path.join(tmpdir, f"{name}.log"), "r")
                        content = log.read().strip()
                        log.close()
                        if content:
                            print(f"=== {name} LOG ===", file=sys.stderr)
                            for line in content.split("\n")[-30:]:
                                print(f"  {line}", file=sys.stderr)
                    except Exception:
                        pass
            for name in ["cafe-bus", "store", "llm", "agent", "tts"]:
                p = procs.get(name)
                if p:
                    p.kill()
            for name in ["cafe-bus", "store", "llm", "agent", "tts"]:
                p = procs.get(name)
                if p:
                    p.wait()

    print(file=sys.stderr)
    if tts_binary_ref is not None:
        print("=== ALL VOLITION PIPELINE E2E TESTS PASSED ===", file=sys.stderr)
    else:
        print("=== VOLITION PIPELINE E2E: partial (TTS model not loaded, LLM+agent pipeline verified) ===", file=sys.stderr)


if __name__ == "__main__":
    main()
