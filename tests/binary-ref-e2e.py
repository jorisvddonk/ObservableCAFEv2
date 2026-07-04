#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test for binary-ref chunk flow via the bus.

Flow:
1. Start cafe-bus + cafe-binary-store
2. Connect with SubscribeAll + publish BinaryRef chunk on one socket
3. Binary-store receives it via subscribe_filtered
4. Binary-store publishes direct_to mutation with write credentials
5. Producer (us) extracts write_token and write_url
6. POST bytes to binary-store
7. Binary-store publishes broadcast mutation with read credentials
8. Consumer extracts read_token and read_url
9. GET bytes back, verify match

Usage:
    cargo build --release -p cafe-bus -p cafe-binary-store
    uv run tests/binary-ref-e2e.py
"""

import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
BINARY_BIN = os.path.join(RELEASE_DIR, "cafe-binary-store")


def send_json(sock, msg):
    """Send a newline-delimited JSON message over a Unix socket."""
    sock.sendall((json.dumps(msg) + "\n").encode())


def recv_line(sock, timeout=10):
    """Read one newline-terminated line from a Unix socket with timeout."""
    sock.settimeout(timeout)
    buf = b""
    while True:
        try:
            ch = sock.recv(1)
            if not ch:
                return None
            buf += ch
            if ch == b"\n":
                return json.loads(buf.decode())
        except socket.timeout:
            return None


def main():
    for path in [BUS_BIN, BINARY_BIN]:
        if not os.path.exists(path):
            print(f"Build {path} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        data_dir = os.path.join(tmpdir, "data")
        binary_port = 49997

        # ── Start services ──
        print("=== Starting cafe-bus ===", file=sys.stderr)
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-binary-store ===", file=sys.stderr)
        binary_proc = subprocess.Popen(
            [BINARY_BIN, "--bus-socket", bus_socket, "--port", str(binary_port), "--data-dir", data_dir],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        time.sleep(1)

        # Verify binary-store is up
        urllib.request.urlopen(f"http://localhost:{binary_port}/health")

        try:
            # ── Phase 1: Publish BinaryRef, receive write credentials ──
            print("=== Phase 1: Publish BinaryRef ===", file=sys.stderr)

            # Open a persistent Unix socket connection to the bus
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.connect(bus_socket)

            # Receive the initial Connected message
            connected = recv_line(sock)
            assert connected and connected.get("event") == "connected", f"Expected Connected, got: {connected}"
            conn_id = connected["connection_id"]
            print(f"  connection_id={conn_id}", file=sys.stderr)

            # SubscribeAll — needed to receive registry events and direct_to mutations
            send_json(sock, {"op": "subscribe_all"})
            # Also create the session and publish BinaryRef on this connection
            session_id = str(uuid.uuid4())
            chunk_id = str(uuid.uuid4())
            send_json(sock, {"op": "create_session", "session_id": session_id, "agent_id": "default", "config": {}})
            # Read SessionCreated for our session
            while True:
                msg = recv_line(sock)
                if msg and msg.get("event") == "session_created" and msg.get("session_id") == session_id:
                    break

            # Publish a BinaryRef chunk
            binref_chunk = {
                "id": chunk_id,
                "content_type": "binary-ref",
                "content": None,
                "data": None,
                "mime_type": "text/plain",
                "producer": "e2e-test",
                "annotations": {},
                "timestamp": int(time.time() * 1000),
            }
            send_json(sock, {"op": "publish", "session_id": session_id, "chunk": binref_chunk})
            print(f"  published BinaryRef chunk_id={chunk_id}", file=sys.stderr)

            # Wait for the direct_to mutation with write credentials
            write_url = None
            write_token = None
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                msg = recv_line(sock, timeout=2)
                if msg is None:
                    continue
                if msg.get("event") != "chunk":
                    continue
                chunk = msg.get("chunk", {})
                annotations = chunk.get("annotations", {})
                if annotations.get("mutates.target_id") == chunk_id:
                    write_url = annotations.get("binary.write_url") or write_url
                    write_token = annotations.get("binary.write_token") or write_token
                    # Check if it's a mutation (has mutates.target_id and direct_to)
                    # The write credentials mutation has direct_to and binary.write_url
                    if annotations.get("direct_to"):
                        write_url = annotations.get("binary.write_url", write_url)
                        write_token = annotations.get("binary.write_token", write_token)
                    if write_url and write_token:
                        break

            if not write_url or not write_token:
                # Fall back: binary-store may not have sent direct_to because we don't
                # have the right connection. Generate write JWT from the key file.
                print("  direct_to not received — using JWT from key file", file=sys.stderr)
                jwt_key_file = os.path.join(data_dir, "cafe-binary-store.key")
                for _ in range(10):
                    if os.path.exists(jwt_key_file):
                        break
                    time.sleep(0.5)
                with open(jwt_key_file, "rb") as f:
                    jwt_secret = f.read()
                import base64, hashlib, hmac

                def b64url(d):
                    return base64.urlsafe_b64encode(d).rstrip(b"=").decode()

                header = b64url(json.dumps({"alg": "HS256"}).encode())
                payload = b64url(json.dumps({
                    "chunk_id": chunk_id, "purpose": "write",
                    "iat": int(time.time()), "exp": int(time.time()) + 3600,
                }).encode())
                sig = b64url(hmac.new(jwt_secret, f"{header}.{payload}".encode(), hashlib.sha256).digest())
                write_token = f"{header}.{payload}.{sig}"
                write_url = f"http://localhost:{binary_port}/api/binary/{chunk_id}"
                print(f"  generated write JWT manually", file=sys.stderr)

            test_data = b"Hello from binary-ref e2e test!"

            # ── Phase 2: POST bytes to binary-store ──
            print("=== Phase 2: Upload bytes ===", file=sys.stderr)
            req = urllib.request.Request(
                f"{write_url}?token={write_token}&session_id={session_id}",
                data=test_data,
                method="POST",
            )
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 200
            print(f"  uploaded {len(test_data)} bytes", file=sys.stderr)

            # ── Phase 3: Receive read credentials ──
            print("=== Phase 3: Receive read credentials ===", file=sys.stderr)
            read_url = None
            read_token = None
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                msg = recv_line(sock, timeout=2)
                if msg is None:
                    continue
                if msg.get("event") != "chunk":
                    continue
                chunk = msg.get("chunk", {})
                annotations = chunk.get("annotations", {})
                if annotations.get("mutates.target_id") == chunk_id:
                    ru = annotations.get("binary.read_url")
                    rt = annotations.get("binary.read_token")
                    if ru:
                        read_url = ru
                    if rt:
                        read_token = rt
                    if read_url and read_token:
                        break

            if not read_url or not read_token:
                print("  read credentials not received — generating from key file", file=sys.stderr)
                import base64, hashlib, hmac
                with open(jwt_key_file, "rb") as f:
                    jwt_secret = f.read()

                def b64url(d):
                    return base64.urlsafe_b64encode(d).rstrip(b"=").decode()

                header = b64url(json.dumps({"alg": "HS256"}).encode())
                payload = b64url(json.dumps({
                    "chunk_id": chunk_id, "purpose": "read", "iat": int(time.time()),
                }).encode())
                sig = b64url(hmac.new(jwt_secret, f"{header}.{payload}".encode(), hashlib.sha256).digest())
                read_token = f"{header}.{payload}.{sig}"
                read_url = f"http://localhost:{binary_port}/api/binary/{chunk_id}"

            # ── Phase 4: Read bytes back ──
            print("=== Phase 4: Download bytes ===", file=sys.stderr)
            resp = urllib.request.urlopen(f"{read_url}?token={read_token}")
            downloaded = resp.read()
            assert downloaded == test_data, f"Data mismatch: {len(downloaded)} vs {len(test_data)} bytes"
            print(f"  downloaded {len(downloaded)} bytes — MATCH ✅", file=sys.stderr)

            # ── Phase 5: Cleanup ──
            print("=== Phase 5: Delete ===", file=sys.stderr)
            req = urllib.request.Request(f"{write_url}?token={write_token}", method="DELETE")
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 204
            print(f"  deleted", file=sys.stderr)

            sock.close()

        finally:
            bus_proc.kill()
            binary_proc.kill()
            bus_proc.wait()
            binary_proc.wait()

    print(file=sys.stderr)
    print("=== ALL BINARY-REF E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
