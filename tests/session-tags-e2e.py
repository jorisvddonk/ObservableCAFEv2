#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: session tag validation (ADR-117).

Flow:
1. Start cafe-bus + cafe-store + cafe-server
2. Bus-level: create a session with initial tags and verify they appear in
   list-sessions and as a `session.tags` annotation chunk in history
3. Bus-level: reject tags with whitespace / empty tags on create-session
   (INVALID_TAGS), and reject them on set-tags without mutating state
4. Bus-level: set-tags replaces tags and an empty set-tags clears them
5. HTTP-level: PATCH /api/sessions/:id/tags accepts valid tags (200) and
   rejects invalid tags (400)

Usage:
    cargo build --release
    uv run tests/session-tags-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")
SERVER_BIN = os.path.join(RELEASE_DIR, "cafe-server")

SERVER_PORT = 49987
TOKEN = "test-admin-token"


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def start_proc(cmd, env, logfile):
    return subprocess.Popen(cmd, env=env, stdout=open(logfile, "w"), stderr=subprocess.STDOUT)


def cli(bus_socket, *args):
    return subprocess.run(
        [CLI, "--bus", bus_socket, *args], capture_output=True, text=True
    )


def cli_ok(bus_socket, *args):
    r = cli(bus_socket, *args)
    assert r.returncode == 0, f"cli {args} failed: {r.stderr}\n{r.stdout}"
    return r.stdout.strip()


def tags_for(sessions, session_id):
    for s in sessions:
        if s.get("session_id") == session_id:
            return s.get("tags")
    raise AssertionError(f"session {session_id} not found in list-sessions")


def list_sessions(bus_socket):
    out = cli_ok(bus_socket, "list-sessions")
    return json.loads(out)


def history(bus_socket, session_id):
    out = cli_ok(bus_socket, "history", session_id)
    return [json.loads(line) for line in out.split("\n") if line.strip()]


def http_request(method, path, body=None):
    url = f"http://localhost:{SERVER_PORT}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {TOKEN}")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            raw = resp.read().decode()
            return resp.status, raw
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def main():
    for name, path in [
        ("cafe-bus", BUS_BIN),
        ("cafe-store", STORE_BIN),
        ("cafe-server", SERVER_BIN),
        ("cafe-cli", CLI),
    ]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["CAFE_DB_PATH"] = db_path
        env["CAFE_ADMIN_TOKEN"] = TOKEN
        env["PORT"] = str(SERVER_PORT)

        print("=== Starting services ===", file=sys.stderr)
        procs = {}
        for key, path in [
            ("bus", BUS_BIN),
            ("store", STORE_BIN),
            ("server", SERVER_BIN),
        ]:
            log = os.path.join(tmpdir, f"{key}.log")
            procs[key] = start_proc([path], env, log)
            time.sleep(1)
        time.sleep(2)

        try:
            # ── Phase 1: create with initial tags (bus) ──
            print("=== Phase 1: create-session with initial tags ===", file=sys.stderr)
            sid = cli_ok(bus_socket, "create-session", "--agent", "default",
                         "--tag", "work", "--tag", "urgent")
            sessions = list_sessions(bus_socket)
            assert tags_for(sessions, sid) == ["work", "urgent"], \
                f"expected initial tags, got {tags_for(sessions, sid)}"
            print(f"  ✅ initial tags persisted for {sid}", file=sys.stderr)

            # ── Phase 2: dual-write annotation chunk in history ──
            print("=== Phase 2: session.tags annotation chunk ===", file=sys.stderr)
            tag_chunks = [
                c for c in history(bus_socket, sid)
                if c.get("annotations", {}).get("session.tags") is not None
            ]
            assert tag_chunks, "expected a session.tags annotation chunk in history"
            assert tag_chunks[-1]["annotations"]["session.tags"] == ["work", "urgent"], \
                f"unexpected tag annotation: {tag_chunks[-1]['annotations']['session.tags']}"
            print("  ✅ session.tags annotation chunk present", file=sys.stderr)

            # ── Phase 3: reject invalid tags on create (bus) ──
            print("=== Phase 3: create-session rejects invalid tags ===", file=sys.stderr)
            for bad in ["bad tag", "", "tab\there"]:
                r = cli(bus_socket, "create-session", "--agent", "default", "--tag", bad)
                combined = (r.stdout + r.stderr)
                assert r.returncode != 0, f"create-session accepted invalid tag {bad!r}"
                assert "INVALID_TAGS" in combined, \
                    f"expected INVALID_TAGS for {bad!r}, got: {combined}"
            print("  ✅ invalid create tags rejected", file=sys.stderr)

            # ── Phase 4: set-tags replaces and clears (bus) ──
            print("=== Phase 4: set-tags replace + clear ===", file=sys.stderr)
            cli_ok(bus_socket, "set-tags", sid, "--tag", "archived")
            assert tags_for(list_sessions(bus_socket), sid) == ["archived"], \
                "set-tags did not replace"
            cli_ok(bus_socket, "set-tags", sid)
            assert tags_for(list_sessions(bus_socket), sid) == [], \
                "set-tags with no tags did not clear"
            print("  ✅ replace + clear work", file=sys.stderr)

            # ── Phase 5: set-tags rejects invalid without mutating ──
            print("=== Phase 5: set-tags rejects invalid tags ===", file=sys.stderr)
            cli_ok(bus_socket, "set-tags", sid, "--tag", "keep")
            r = cli(bus_socket, "set-tags", sid, "--tag", "not ok")
            combined = (r.stdout + r.stderr)
            assert r.returncode != 0, "set-tags accepted an invalid tag"
            assert "INVALID_TAGS" in combined, f"expected INVALID_TAGS, got: {combined}"
            assert tags_for(list_sessions(bus_socket), sid) == ["keep"], \
                "invalid set-tags mutated tags"
            print("  ✅ invalid set-tags rejected, tags unchanged", file=sys.stderr)

            # ── Phase 6: HTTP PATCH accepts valid tags ──
            print("=== Phase 6: HTTP PATCH valid tags ===", file=sys.stderr)
            status, body = http_request("PATCH", f"/api/sessions/{sid}/tags",
                                        {"tags": ["http-ok", "x"]})
            assert status == 200, f"PATCH valid tags returned {status}: {body}"
            assert tags_for(list_sessions(bus_socket), sid) == ["http-ok", "x"], \
                "HTTP tags did not apply"
            print("  ✅ HTTP valid tags applied", file=sys.stderr)

            # ── Phase 7: HTTP PATCH rejects invalid tags ──
            print("=== Phase 7: HTTP PATCH invalid tags ===", file=sys.stderr)
            status, body = http_request("PATCH", f"/api/sessions/{sid}/tags",
                                        {"tags": ["bad tag"]})
            assert status == 400, f"PATCH invalid tags returned {status}: {body}"
            assert tags_for(list_sessions(bus_socket), sid) == ["http-ok", "x"], \
                "HTTP invalid tags mutated state"
            print("  ✅ HTTP invalid tags rejected with 400", file=sys.stderr)

            print("\n=== ALL SESSION TAG TESTS PASSED ===", file=sys.stderr)

        finally:
            for key in ["server", "store", "bus"]:
                p = procs.pop(key, None)
                if p:
                    p.terminate()
                    try:
                        p.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        p.kill()
                        p.wait()


if __name__ == "__main__":
    main()
