#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: cafe-bus + cafe-binary-store + cafe-cli.

Usage:
    cargo build --release
    uv run tests/binary-store-e2e.py

Starts cafe-bus and cafe-binary-store, exercises operations
via cafe-cli and direct HTTP to the binary-store.
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
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        binary_port = 49998
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
            resp = urllib.request.urlopen(f"http://localhost:{binary_port}/health")
            assert json.loads(resp.read())["status"] == "ok"
            print("  health OK", file=sys.stderr)

            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            session_id = r.stdout.strip()

            r = run([CLI, "--bus", bus_socket, "publish", session_id, "--text", "hello"])
            assert r.returncode == 0

            r = run([CLI, "--bus", bus_socket, "publish", session_id, "--binary-ref", "--mime", "audio/wav", "--transient"])
            assert r.returncode == 0

            r = run([CLI, "--bus", bus_socket, "history", session_id])
            assert r.returncode == 0
            chunks = [json.loads(line) for line in r.stdout.strip().split("\n") if line.strip()]
            assert len(chunks) >= 1
            print(f"  history: {len(chunks)} chunk(s)", file=sys.stderr)

            # Binary-store HTTP API
            jwt_secret_file = os.path.join(data_dir, "cafe-binary-store.key")
            for _ in range(10):
                if os.path.exists(jwt_secret_file):
                    break
                time.sleep(0.5)
            with open(jwt_secret_file, "rb") as f:
                jwt_secret = f.read()

            import base64, hashlib, hmac, time as time_mod
            def b64url(d): return base64.urlsafe_b64encode(d).rstrip(b"=").decode()
            def make_jwt(cid, purpose, exp=None):
                h = b64url(json.dumps({"alg":"HS256"}).encode())
                p = b64url(json.dumps({"chunk_id":cid,"purpose":purpose,"iat":int(time_mod.time()), **({"exp":exp} if exp else {})}).encode())
                s = b64url(hmac.new(jwt_secret, f"{h}.{p}".encode(), hashlib.sha256).digest())
                return f"{h}.{p}.{s}"

            wt = make_jwt("e2e-chunk", "write", int(time_mod.time()) + 3600)
            rt = make_jwt("e2e-chunk", "read")

            req = urllib.request.Request(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={wt}", data=b"Hello!", method="POST")
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 200
            print("  write OK", file=sys.stderr)

            resp = urllib.request.urlopen(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={rt}")
            assert resp.read() == b"Hello!"
            print("  read OK", file=sys.stderr)

            req = urllib.request.Request(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={rt}")
            req.add_header("Range", "bytes=0-4")
            resp = urllib.request.urlopen(req)
            assert resp.read().startswith(b"Hello")
            print("  range OK", file=sys.stderr)

            req = urllib.request.Request(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={wt}", method="DELETE")
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 204
            print("  delete OK", file=sys.stderr)

            try:
                urllib.request.urlopen(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={rt}")
                assert False
            except urllib.error.HTTPError as e:
                assert e.code == 404
            print("  404 after delete OK", file=sys.stderr)

        finally:
            bus_proc.kill()
            binary_proc.kill()
            bus_proc.wait()
            binary_proc.wait()

    print(file=sys.stderr)
    print("=== ALL BINARY-STORE TESTS PASSED ===", file=sys.stderr)

if __name__ == "__main__":
    main()
