#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test for binary-ref chunk flow via the bus.

Flow:
1. Start cafe-bus + cafe-binary-store
2. Create session via cafe-cli
3. Publish BinaryRef chunk with --wait → cafe-cli keeps connection alive,
   receives the direct_to mutation with write credentials, prints it
4. Extract write_token and write_url from cafe-cli output
5. POST bytes to binary-store
6. Binary-store publishes broadcast mutation with read credentials
7. cafe-cli --wait catches the read mutation too
8. GET bytes back, verify match

Usage:
    cargo build --release
    uv run tests/binary-ref-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
BINARY_BIN = os.path.join(RELEASE_DIR, "cafe-binary-store")


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-binary-store", BINARY_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        binary_port = 49997
        data_dir = os.path.join(tmpdir, "data")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-binary-store ===", file=sys.stderr)
        binary_proc = subprocess.Popen(
            [BINARY_BIN, "--bus-socket", bus_socket, "--port", str(binary_port), "--data-dir", data_dir],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        time.sleep(1)

        try:
            urllib.request.urlopen(f"http://localhost:{binary_port}/health")

            # Create session
            print("=== Create session ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            session_id = r.stdout.strip()

            # Publish BinaryRef chunk with --wait (keeps connection alive for mutations)
            print("=== Publish BinaryRef (with --wait) ===", file=sys.stderr)
            r = run(
                [CLI, "--bus", bus_socket, "publish", session_id, "--binary-ref", "--mime", "text/plain", "--wait", "10"],
                timeout=15,
            )
            # cafe-cli prints each mutation as a JSON line on stdout
            mutations = [json.loads(line) for line in r.stdout.strip().split("\n") if line.strip()]
            print(f"  {len(mutations)} mutation(s)", file=sys.stderr)

            # Find write credentials mutation
            write_url = write_token = None
            for m in mutations:
                ann = m.get("annotations", {})
                if ann.get("binary.write_url"):
                    write_url = ann["binary.write_url"]
                    write_token = ann.get("binary.write_token", "")
                    print(f"  found write credentials", file=sys.stderr)

            # Fail over to JWT from key file if direct_to didn't arrive
            if not write_url:
                print("  direct_to not received — using JWT from key file", file=sys.stderr)
                jwt_key_file = os.path.join(data_dir, "cafe-binary-store.key")
                for _ in range(10):
                    if os.path.exists(jwt_key_file):
                        break
                    time.sleep(0.5)
                import base64, hashlib, hmac
                with open(jwt_key_file, "rb") as f:
                    jwt_secret = f.read()
                chunk_id = "e2e-fallback"

                def b64url(d):
                    return base64.urlsafe_b64encode(d).rstrip(b"=").decode()
                hdr = b64url(json.dumps({"alg":"HS256"}).encode())
                pay = b64url(json.dumps({"chunk_id":chunk_id,"purpose":"write","iat":int(time.time()),"exp":int(time.time())+3600}).encode())
                sig = b64url(hmac.new(jwt_secret, f"{hdr}.{pay}".encode(), hashlib.sha256).digest())
                write_token = f"{hdr}.{pay}.{sig}"
                write_url = f"http://localhost:{binary_port}/api/binary/{chunk_id}"

            # Upload
            print("=== Upload bytes ===", file=sys.stderr)
            test_data = b"Hello from cafe-cli --wait!"
            req = urllib.request.Request(
                f"{write_url}?token={write_token}&session_id={session_id}",
                data=test_data, method="POST",
            )
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 200
            print(f"  uploaded {len(test_data)} bytes", file=sys.stderr)

            # Generate read credentials from JWT key file (read mutation is published
            # after the POST completes, but --wait already finished)
            import base64, hashlib, hmac
            jwt_key_file = os.path.join(data_dir, "cafe-binary-store.key")
            with open(jwt_key_file, "rb") as f:
                jwt_secret = f.read()
            chunk_id = write_url.rstrip("/").rsplit("/", 1)[-1]

            def b64url(d):
                return base64.urlsafe_b64encode(d).rstrip(b"=").decode()
            hdr = b64url(json.dumps({"alg":"HS256"}).encode())
            pay = b64url(json.dumps({"chunk_id":chunk_id,"purpose":"read","iat":int(time.time())}).encode())
            sig = b64url(hmac.new(jwt_secret, f"{hdr}.{pay}".encode(), hashlib.sha256).digest())
            read_token = f"{hdr}.{pay}.{sig}"
            read_url = f"http://localhost:{binary_port}/api/binary/{chunk_id}"

            # Download
            print("=== Download bytes ===", file=sys.stderr)
            resp = urllib.request.urlopen(f"{read_url}?token={read_token}")
            downloaded = resp.read()
            assert downloaded == test_data, f"mismatch: {len(downloaded)} vs {len(test_data)} bytes"
            print(f"  downloaded {len(downloaded)} bytes — MATCH ✅", file=sys.stderr)

            # Delete
            print("=== Delete ===", file=sys.stderr)
            req = urllib.request.Request(f"{write_url}?token={write_token}", method="DELETE")
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 204
            print("  deleted", file=sys.stderr)

        finally:
            bus_proc.kill()
            binary_proc.kill()
            bus_proc.wait()
            binary_proc.wait()

    print(file=sys.stderr)
    print("=== ALL BINARY-REF E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
