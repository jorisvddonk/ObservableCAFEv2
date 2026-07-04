#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test for binary-ref chunk flow via the bus.

Write credentials via direct_to mutation (--wait), read credentials
from session history (non-transient broadcast mutation).

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

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-binary-store ===", file=sys.stderr)
        binary_proc = subprocess.Popen(
            [BINARY_BIN, "--bus-socket", bus_socket, "--port", str(binary_port), "--data-dir", tmpdir],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        time.sleep(1)

        try:
            urllib.request.urlopen(f"http://localhost:{binary_port}/health")

            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            session_id = r.stdout.strip()

            # Publish BinaryRef with --wait to capture write credentials via direct_to
            print("=== Publish BinaryRef (--wait) ===", file=sys.stderr)
            r = run(
                [CLI, "--bus", bus_socket, "publish", session_id, "--binary-ref", "--mime", "text/plain", "--wait", "5"],
                timeout=10,
            )
            mutations = [json.loads(l) for l in r.stdout.strip().split("\n") if l.strip()]
            write_url = write_token = None
            for m in mutations:
                ann = m.get("annotations", {})
                if ann.get("cafe.binary.write_url"):
                    write_url = ann["cafe.binary.write_url"]
                    write_token = ann.get("cafe.binary.write_token", "")
                    print(f"  write credentials via direct_to ✅", file=sys.stderr)

            assert write_url, "no write credentials in mutations"
            chunk_id = write_url.rstrip("/").rsplit("/", 1)[-1]

            # Upload
            print("=== Upload bytes ===", file=sys.stderr)
            test_data = b"Hello from binary-ref!"
            req = urllib.request.Request(
                f"{write_url}?token={write_token}&session_id={session_id}",
                data=test_data, method="POST",
            )
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 200
            print(f"  uploaded {len(test_data)} bytes", file=sys.stderr)

            # Read credentials from session history (non-transient mutation)
            print("=== Read credentials from history ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "history", session_id])
            assert r.returncode == 0
            chunks = [json.loads(l) for l in r.stdout.strip().split("\n") if l.strip()]

            read_url = read_token = None
            for c in chunks:
                if c.get("annotations", {}).get("cafe.mutates.target_id") == chunk_id:
                    ru = c.get("annotations", {}).get("cafe.binary.read_url")
                    rt = c.get("annotations", {}).get("cafe.binary.read_token")
                    if ru:
                        read_url = ru
                    if rt:
                        read_token = rt

            assert read_url, "no read credentials in history"
            print(f"  read credentials from history ✅", file=sys.stderr)

            # Download
            print("=== Download bytes ===", file=sys.stderr)
            resp = urllib.request.urlopen(f"{read_url}?token={read_token}")
            downloaded = resp.read()
            assert downloaded == test_data, f"mismatch: {len(downloaded)} vs {len(test_data)}"
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

    print("\n=== ALL BINARY-REF E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
