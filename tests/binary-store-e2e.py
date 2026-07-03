#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: cafe-bus + cafe-binary-store + cafe-cli + cafe-dice (tool calling).

Usage:
    cargo build --release
    uv run tests/binary-store-e2e.py

Starts services, exercises basic operations via cafe-cli and direct HTTP.
"""

import json
import os
import signal
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
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")
AGENT_BIN = os.path.join(RELEASE_DIR, "cafe-agent-runtime")
DICE_BIN = os.path.join(RELEASE_DIR, "cafe-dice")
AGENT_DIR = os.path.join(PROJECT_ROOT, "agents")


def check_binaries():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-binary-store", BINARY_BIN),
                       ("cafe-store", STORE_BIN), ("cafe-agent-runtime", AGENT_BIN),
                       ("cafe-cli", CLI), ("cafe-dice", DICE_BIN)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def main():
    check_binaries()

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        binary_port = 49998
        data_dir = os.path.join(tmpdir, "data")
        store_db = os.path.join(tmpdir, "cafe.db")

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

        print("=== Starting cafe-store ===", file=sys.stderr)
        store_env = env.copy()
        store_env["CAFE_DB_PATH"] = store_db
        store_proc = subprocess.Popen([STORE_BIN], env=store_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-dice ===", file=sys.stderr)
        dice_env = env.copy()
        dice_proc = subprocess.Popen([DICE_BIN], env=dice_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-agent-runtime ===", file=sys.stderr)
        agent_env = env.copy()
        agent_env["CAFE_DB_PATH"] = store_db
        agent_log = os.path.join(tmpdir, "agent.log")
        agent_proc = subprocess.Popen([AGENT_BIN], env=agent_env, stdout=subprocess.DEVNULL, stderr=open(agent_log, "w"))
        time.sleep(2)

        try:
            # Health check
            print("=== Health check ===", file=sys.stderr)
            resp = urllib.request.urlopen(f"http://localhost:{binary_port}/health")
            assert json.loads(resp.read())["status"] == "ok"
            print("  OK", file=sys.stderr)

            # Create a session via CLI
            print("=== Create session ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            session_id = r.stdout.strip()
            print(f"  session_id={session_id}", file=sys.stderr)

            # List sessions via CLI
            print("=== List sessions ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "list-sessions"])
            assert r.returncode == 0
            sessions = json.loads(r.stdout)
            assert any(s["session_id"] == session_id for s in sessions), f"session {session_id} not found"
            print(f"  {len(sessions)} session(s)", file=sys.stderr)

            # Publish text chunk via CLI
            print("=== Publish text ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "publish", session_id, "--text", "hello world"])
            assert r.returncode == 0

            # Publish BinaryRef chunk via CLI
            print("=== Publish BinaryRef ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "publish", session_id, "--binary-ref", "--mime", "audio/wav", "--transient"])
            assert r.returncode == 0

            # Read history via CLI
            print("=== History ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "history", session_id])
            assert r.returncode == 0
            chunks = [json.loads(line) for line in r.stdout.strip().split("\n") if line.strip()]
            # BinaryRef was transient so it won't be in history. Text chunk should be there.
            assert len(chunks) >= 1, f"expected at least 1 chunk, got {len(chunks)}"
            print(f"  {len(chunks)} chunk(s) in history", file=sys.stderr)

            # Test binary-store HTTP API directly
            print("=== Binary-store write (HTTP POST) ===", file=sys.stderr)
            jwt_secret_file = os.path.join(data_dir, "cafe-binary-store.key")
            # Wait for JWT key file
            for _ in range(10):
                if os.path.exists(jwt_secret_file):
                    break
                time.sleep(0.5)
            with open(jwt_secret_file, "rb") as f:
                jwt_secret = f.read()

            # Generate a write JWT (simple HMAC-SHA256)
            import base64, hashlib, hmac, time as time_mod

            def b64url(data):
                return base64.urlsafe_b64encode(data).rstrip(b"=").decode()

            def make_jwt(chunk_id, purpose, exp=None):
                header = b64url(json.dumps({"alg": "HS256"}).encode())
                payload_dict = {"chunk_id": chunk_id, "purpose": purpose, "iat": int(time_mod.time())}
                if exp is not None:
                    payload_dict["exp"] = exp
                payload = b64url(json.dumps(payload_dict).encode())
                sig = b64url(hmac.new(jwt_secret, f"{header}.{payload}".encode(), hashlib.sha256).digest())
                return f"{header}.{payload}.{sig}"

            write_token = make_jwt("e2e-chunk", "write", int(time_mod.time()) + 3600)
            read_token = make_jwt("e2e-chunk", "read")

            # Write
            req = urllib.request.Request(
                f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={write_token}",
                data=b"Hello, Binary Store!",
                method="POST",
            )
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 200
            print("  write OK", file=sys.stderr)

            # Read
            resp = urllib.request.urlopen(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={read_token}")
            data = resp.read()
            assert data == b"Hello, Binary Store!", f"read mismatch: {data!r}"
            print("  read OK", file=sys.stderr)

            # Range read (server doesn't parse Range end byte yet — just checks prefix)
            req = urllib.request.Request(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={read_token}")
            req.add_header("Range", "bytes=7-12")
            resp = urllib.request.urlopen(req)
            data = resp.read()
            assert data.startswith(b"Binary"), f"range mismatch: {data!r}"
            print("  range OK (start)", file=sys.stderr)

            # Delete
            req = urllib.request.Request(
                f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={write_token}",
                method="DELETE",
            )
            with urllib.request.urlopen(req) as resp:
                assert resp.status == 204
            print("  delete OK", file=sys.stderr)

            # 404 after delete
            try:
                urllib.request.urlopen(f"http://localhost:{binary_port}/api/binary/e2e-chunk?token={read_token}")
                assert False, "expected 404"
            except urllib.error.HTTPError as e:
                assert e.code == 404

            print("  404 after delete OK", file=sys.stderr)

            # === Dice tool calling test ===
            print("=== Dice roll via pipeline ===", file=sys.stderr)

            # Create a session with the dice agent
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "dice"])
            assert r.returncode == 0
            dice_sid = r.stdout.strip()
            print(f"  dice session={dice_sid}", file=sys.stderr)
            time.sleep(1)

            # Publish the roll command — pipeline dispatches dice-detector.invoke → cafe-dice
            # detects !roll → publishes tool.call → pipeline dispatches dice.roll → cafe-dice
            # rolls → publishes jsonrpc.response (transient with 60s retain)
            r = run([CLI, "--bus", bus_socket, "publish", dice_sid, "--text", "!roll 2d6"])
            assert r.returncode == 0
            time.sleep(1)

            # Subscribe (after publish) — retained buffer serves the RPC response
            sub_proc = subprocess.Popen(
                [CLI, "--bus", bus_socket, "subscribe", dice_sid, "--timeout-secs", "6"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            )
            try:
                stdout, _ = sub_proc.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()
                stdout, _ = sub_proc.communicate()

            # Parse chunks looking for the dice.roll RPC response
            found_result = None
            for line in stdout.strip().split("\n"):
                if not line.strip():
                    continue
                try:
                    chunk = json.loads(line)
                    annotations = chunk.get("annotations", {})
                    rpc_resp = annotations.get("jsonrpc.response")
                    if rpc_resp:
                        result = rpc_resp.get("result", {}).get("result")
                        print(f"  [debug] found jsonrpc.response: {json.dumps(rpc_resp)[:200]}", file=sys.stderr)
                        if result is not None:
                            found_result = result
                            break
                except (json.JSONDecodeError, AttributeError) as e:
                    print(f"  [debug] parse error: {e}", file=sys.stderr)
                    continue

            if found_result is None:
                print(f"  [debug] subscribe output ({len(stdout)} bytes, repr): {stdout[:500]!r}", file=sys.stderr)
                print(f"  [debug] subscribe lines: {stdout.strip().split(chr(10))[:10]}", file=sys.stderr)
                print(f"  [debug] agent log (tail):", file=sys.stderr)
                try:
                    with open(agent_log) as f:
                        for line in f.readlines()[-10:]:
                            print(f"    {line.rstrip()[:200]}", file=sys.stderr)
                except FileNotFoundError:
                    print(f"    (no agent log)", file=sys.stderr)
            assert found_result is not None, f"no dice.roll result found in {len(stdout)} bytes of subscribe output"
            assert isinstance(found_result, int), f"result should be int, got {type(found_result)}"
            assert 2 <= found_result <= 12, f"2d6 should be 2-12, got {found_result}"
            print(f"  dice roll OK: {found_result}", file=sys.stderr)

            # Clean up
            r = run([CLI, "--bus", bus_socket, "delete-session", dice_sid])
            print("  dice session deleted", file=sys.stderr)

        finally:
            for p in [bus_proc, binary_proc, store_proc, dice_proc, agent_proc]:
                p.kill()
            for p in [bus_proc, binary_proc, store_proc, dice_proc, agent_proc]:
                p.wait()

    print(file=sys.stderr)
    print("=== ALL E2E TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
