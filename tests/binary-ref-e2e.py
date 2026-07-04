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
3. Publish BinaryRef chunk with --wait in the BACKGROUND
4. Read write credentials from --wait's stdout (direct_to mutation)
5. POST bytes to binary-store (while --wait is still running)
6. Read read credentials from --wait's stdout (broadcast mutation)
7. GET bytes back, verify match
8. Delete

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
            r = subprocess.run(
                [CLI, "--bus", bus_socket, "create-session", "--agent", "default"],
                capture_output=True, text=True,
            )
            assert r.returncode == 0
            session_id = r.stdout.strip()

            # Publish BinaryRef with --wait in BACKGROUND (30s timeout)
            # This keeps the connection alive to receive BOTH the write mutation
            # (published immediately) and the read mutation (published after POST).
            print("=== Publish BinaryRef (background --wait) ===", file=sys.stderr)
            wait_proc = subprocess.Popen(
                [CLI, "--bus", bus_socket, "publish", session_id, "--binary-ref", "--mime", "text/plain", "--wait", "30"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            )
            time.sleep(1)

            # Read the write credentials mutation from --wait's stdout
            write_url = write_token = None
            deadline = time.time() + 10
            while time.time() < deadline:
                line = wait_proc.stdout.readline()
                if not line:
                    continue
                mut = json.loads(line.strip())
                ann = mut.get("annotations", {})
                if ann.get("binary.write_url"):
                    write_url = ann["binary.write_url"]
                    write_token = ann.get("binary.write_token", "")
                    print(f"  got write credentials via direct_to ✅", file=sys.stderr)
                    break

            if not write_url:
                raise AssertionError("No write credentials received via direct_to")

            # Upload bytes (while --wait is still running)
            print("=== Upload bytes ===", file=sys.stderr)
            test_data = b"Hello from binary-ref e2e!"
            req = urllib.request.Request(
                f"{write_url}?token={write_token}&session_id={session_id}",
                data=test_data, method="POST",
            )
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 200
            print(f"  uploaded {len(test_data)} bytes", file=sys.stderr)

            # Read the read credentials mutation from --wait's stdout
            read_url = read_token = None
            deadline = time.time() + 10
            while time.time() < deadline:
                line = wait_proc.stdout.readline()
                if not line:
                    continue
                mut = json.loads(line.strip())
                ann = mut.get("annotations", {})
                if ann.get("binary.read_url"):
                    read_url = ann["binary.read_url"]
                    read_token = ann.get("binary.read_token", "")
                    print(f"  got read credentials via broadcast mutation ✅", file=sys.stderr)
                    break

            if not read_url:
                raise AssertionError("No read credentials received via bus")

            # Download bytes using read credentials
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
            try:
                wait_proc.kill()
            except: pass
            bus_proc.wait()
            binary_proc.wait()

    print(file=sys.stderr)
    print("=== ALL BINARY-REF E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
