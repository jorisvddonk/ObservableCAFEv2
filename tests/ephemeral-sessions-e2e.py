#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Ephemeral sessions e2e test.

Tests:
1. Ephemeral session (keepalive=0, count_role="test") — deleted immediately
   when the last matching subscriber disconnects
2. Ephemeral session (keepalive=2, count_role="test") — survives 2s after
   last matching subscriber disconnects
3. Persistent session (no ephemeral config) — survives subscriber disconnect
4. Role filtering: subscriber without matching role doesn't count toward lifecycle

Usage:
    cargo build --release
    uv run tests/ephemeral-sessions-e2e.py
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import threading

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")


def run_cli(args, **kwargs):
    """Run cafe-cli and return CompletedProcess."""
    cmd = [os.path.join(RELEASE_DIR, "cafe-cli"), "--bus", BUS_SOCKET] + args
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


class BusConnection:
    """A raw Unix socket connection to cafe-bus with JSON-line framing."""

    def __init__(self, socket_path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(socket_path)
        self.buf = b""
        # Consume the initial Connected message
        msg = self.read_msg()
        assert msg.get("event") == "connected", f"expected Connected, got {msg}"

    def send_msg(self, msg: dict):
        """Send a JSON-line message."""
        data = json.dumps(msg, separators=(",", ":")) + "\n"
        self.sock.sendall(data.encode())

    def read_msg(self, timeout: float | None = 5.0) -> dict:
        """Read one JSON-line message with timeout."""
        self.sock.settimeout(timeout)
        while b"\n" not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("bus closed connection")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line)

    def close(self):
        self.sock.close()


def test_ephemeral_immediate_deletion():
    """Ephemeral session with keepalive=0 is deleted immediately on disconnect."""
    print("\n=== Test 1: Ephemeral with keepalive=0 ===", file=sys.stderr)

    conn = BusConnection(BUS_SOCKET)

    # Set connection role
    conn.send_msg({"op": "set_meta", "role": "test"})

    # Create ephemeral session with keepalive=0, count_role="test"
    sid = "e2e-ephemeral-0"
    conn.send_msg({
        "op": "create_session",
        "session_id": sid,
        "agent_id": "e2e-test",
        "config": {
            "ephemeral": {
                "keepalive_secs": 0,
                "count_role": "test",
            }
        },
    })
    resp = conn.read_msg()
    assert resp.get("event") == "session_created", f"expected session_created, got {resp}"
    print(f"  created ephemeral session {sid}", file=sys.stderr)

    # Subscribe (this registers as a counted subscriber with role "test")
    conn.send_msg({"op": "subscribe", "session_id": sid})
    resp = conn.read_msg()
    assert resp.get("event") == "history_complete", f"expected history_complete, got {resp}"

    # Verify session exists via list-sessions
    r = run_cli(["list-sessions"])
    assert sid in r.stdout, f"session {sid} should exist before disconnect"
    print(f"  session exists before disconnect ✅", file=sys.stderr)

    # Disconnect — dropping the socket removes the only "test"-role subscriber
    conn.close()
    time.sleep(0.5)

    # Verify session is gone
    r = run_cli(["list-sessions"])
    if sid in r.stdout:
        print(f"  FAIL: session {sid} still exists after disconnect", file=sys.stderr)
        return False
    print(f"  session deleted after disconnect ✅", file=sys.stderr)
    return True


def test_ephemeral_grace_period():
    """Ephemeral session with keepalive=2 survives 2s, then is deleted."""
    print("\n=== Test 2: Ephemeral with keepalive=2 ===", file=sys.stderr)

    conn = BusConnection(BUS_SOCKET)
    conn.send_msg({"op": "set_meta", "role": "test"})

    sid = "e2e-ephemeral-2"
    conn.send_msg({
        "op": "create_session",
        "session_id": sid,
        "agent_id": "e2e-test",
        "config": {
            "ephemeral": {
                "keepalive_secs": 2,
                "count_role": "test",
            }
        },
    })
    resp = conn.read_msg()
    assert resp.get("event") == "session_created", f"expected session_created, got {resp}"

    conn.send_msg({"op": "subscribe", "session_id": sid})
    resp = conn.read_msg()
    assert resp.get("event") == "history_complete"

    # Disconnect — timer starts
    conn.close()

    # Session should still exist shortly after disconnect (within grace period)
    time.sleep(0.5)
    r = run_cli(["list-sessions"])
    if sid not in r.stdout:
        print(f"  FAIL: session {sid} deleted too early (within grace period)", file=sys.stderr)
        return False
    print(f"  session alive after 0.5s (within grace period) ✅", file=sys.stderr)

    # After the full grace period + buffer, session should be gone
    time.sleep(2.5)
    r = run_cli(["list-sessions"])
    if sid in r.stdout:
        print(f"  FAIL: session {sid} still exists after grace period expired", file=sys.stderr)
        return False
    print(f"  session deleted after grace period ✅", file=sys.stderr)
    return True


def test_persistent_session_survives():
    """Persistent session (no ephemeral config) survives subscriber disconnect."""
    print("\n=== Test 3: Persistent session survives disconnect ===", file=sys.stderr)

    conn = BusConnection(BUS_SOCKET)
    conn.send_msg({"op": "set_meta", "role": "test"})

    sid = "e2e-persistent"
    conn.send_msg({
        "op": "create_session",
        "session_id": sid,
        "agent_id": "e2e-test",
        "config": {},
    })
    resp = conn.read_msg()
    assert resp.get("event") == "session_created", f"expected session_created, got {resp}"

    conn.send_msg({"op": "subscribe", "session_id": sid})
    resp = conn.read_msg()
    assert resp.get("event") == "history_complete"

    # Disconnect — no ephemeral config, so session persists
    conn.close()
    time.sleep(1)

    r = run_cli(["list-sessions"])
    if sid not in r.stdout:
        print(f"  FAIL: persistent session {sid} was deleted on disconnect", file=sys.stderr)
        return False
    print(f"  persistent session survives ✅", file=sys.stderr)
    return True


def test_role_filtering():
    """Subscriber without matching role doesn't prevent deletion of ephemeral session."""
    print("\n=== Test 4: Role filtering ===", file=sys.stderr)

    sid = "e2e-role-filter"

    # Connect with the matching role ("owner") and subscribe
    owner_conn = BusConnection(BUS_SOCKET)
    owner_conn.send_msg({"op": "set_meta", "role": "owner"})

    owner_conn.send_msg({
        "op": "create_session",
        "session_id": sid,
        "agent_id": "e2e-test",
        "config": {
            "ephemeral": {
                "keepalive_secs": 0,
                "count_role": "owner",
            }
        },
    })
    resp = owner_conn.read_msg()
    assert resp.get("event") == "session_created"

    owner_conn.send_msg({"op": "subscribe", "session_id": sid})
    resp = owner_conn.read_msg()
    assert resp.get("event") == "history_complete"

    # Verify session exists
    r = run_cli(["list-sessions"])
    assert sid in r.stdout

    # Now add a second subscriber with role=None (like an internal system)
    system_conn = BusConnection(BUS_SOCKET)
    system_conn.send_msg({"op": "subscribe", "session_id": sid})
    resp = system_conn.read_msg()
    assert resp.get("event") == "history_complete"
    print(f"  session has both owner and None-role subscribers ✅", file=sys.stderr)

    # Disconnect the None-role subscriber — session should survive (owner still there)
    system_conn.close()
    time.sleep(0.5)
    r = run_cli(["list-sessions"])
    if sid not in r.stdout:
        print(f"  FAIL: session deleted when only None-role subscriber left", file=sys.stderr)
        return False
    print(f"  session survives after None-role subscriber disconnect (owner still connected) ✅", file=sys.stderr)

    # Now disconnect the owner — counted subscribers = 0, session deleted
    owner_conn.close()
    time.sleep(0.5)
    r = run_cli(["list-sessions"])
    if sid in r.stdout:
        print(f"  FAIL: session still exists after owner disconnected", file=sys.stderr)
        return False
    print(f"  session deleted after owner disconnected (None-role subscriber not counted) ✅", file=sys.stderr)
    return True


def test_reconnect_within_grace():
    """Reconnecting within the grace period cancels deletion."""
    print("\n=== Test 5: Reconnect within grace period ===", file=sys.stderr)

    conn = BusConnection(BUS_SOCKET)
    conn.send_msg({"op": "set_meta", "role": "test"})

    sid = "e2e-reconnect"
    conn.send_msg({
        "op": "create_session",
        "session_id": sid,
        "agent_id": "e2e-test",
        "config": {
            "ephemeral": {
                "keepalive_secs": 5,
                "count_role": "test",
            }
        },
    })
    resp = conn.read_msg()
    assert resp.get("event") == "session_created"

    conn.send_msg({"op": "subscribe", "session_id": sid})
    resp = conn.read_msg()
    assert resp.get("event") == "history_complete"

    # Disconnect — 5s timer starts
    conn.close()
    time.sleep(1)

    # Verify session still alive during grace period
    r = run_cli(["list-sessions"])
    assert sid in r.stdout, f"session should be alive during grace period"
    print(f"  session alive 1s after disconnect ✅", file=sys.stderr)

    # Reconnect with matching role and re-subscribe
    conn2 = BusConnection(BUS_SOCKET)
    conn2.send_msg({"op": "set_meta", "role": "test"})
    conn2.send_msg({"op": "subscribe", "session_id": sid})
    resp = conn2.read_msg()
    assert resp.get("event") == "history_complete"

    # Wait past the original timer (5s from first disconnect)
    time.sleep(4)

    # Session should still exist because re-subscription canceled the timer
    r = run_cli(["list-sessions"])
    if sid not in r.stdout:
        print(f"  FAIL: session deleted despite re-subscription within grace period", file=sys.stderr)
        return False
    print(f"  session survived past original timer thanks to re-subscription ✅", file=sys.stderr)

    # Now disconnect the reconnected subscriber
    conn2.close()
    time.sleep(0.5)

    # New timer started on second disconnect (keepalive 5s), but we only wait 0.5s
    r = run_cli(["list-sessions"])
    if sid not in r.stdout:
        print(f"  FAIL: session deleted too early after second disconnect", file=sys.stderr)
        return False
    print(f"  session alive after second disconnect (within new grace period) ✅", file=sys.stderr)

    return True


def cleanup_old_sessions():
    """Delete any lingering e2e sessions from previous runs."""
    r = run_cli(["list-sessions"])
    for line in r.stdout.strip().split("\n"):
        try:
            sessions = json.loads(line)
            if isinstance(sessions, list):
                for s in sessions:
                    sid = s.get("session_id", "")
                    if sid.startswith("e2e-"):
                        run_cli(["delete-session", sid])
        except json.JSONDecodeError:
            pass


def main():
    global BUS_SOCKET

    if not os.path.exists(BUS_BIN):
        print(f"Build cafe-bus first: cargo build --release", file=sys.stderr)
        sys.exit(1)
    if not os.path.exists(os.path.join(RELEASE_DIR, "cafe-cli")):
        print(f"Build cafe-cli first: cargo build --release", file=sys.stderr)
        sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        BUS_SOCKET = os.path.join(tmpdir, "cafe-bus.sock")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = BUS_SOCKET

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen(
            [BUS_BIN],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(1)

        try:
            cleanup_old_sessions()

            tests = [
                ("Immediate deletion (keepalive=0)", test_ephemeral_immediate_deletion),
                ("Grace period (keepalive=2)", test_ephemeral_grace_period),
                ("Persistent session survives", test_persistent_session_survives),
                ("Role filtering", test_role_filtering),
                ("Reconnect within grace period", test_reconnect_within_grace),
            ]

            passed = 0
            failed = 0
            for name, test_fn in tests:
                try:
                    if test_fn():
                        passed += 1
                    else:
                        failed += 1
                except Exception as e:
                    print(f"  EXCEPTION: {e}", file=sys.stderr)
                    import traceback
                    traceback.print_exc(file=sys.stderr)
                    failed += 1

            print(f"\n{'='*40}", file=sys.stderr)
            print(f"Results: {passed} passed, {failed} failed", file=sys.stderr)
            if failed > 0:
                sys.exit(1)
            print("ALL EPHEMERAL SESSION E2E TESTS PASSED", file=sys.stderr)

        finally:
            bus_proc.kill()
            bus_proc.wait()


if __name__ == "__main__":
    main()
