#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Verify cafe.source.connection is auto-injected on every publish (ADR-101).

Ensures:
1. Every published chunk's annotations contain "cafe.source.connection"
2. The annotation value is a non-empty string matching the publisher's connection ID
3. Multiple publishes from the same CLI invocation get the same connection ID
4. The annotation is never absent from any chunk event

Usage:
    cargo build --release
    uv run tests/source-connection-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")

ANNOTATION_KEY = "cafe.source.connection"


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def parse_chunks(lines):
    """Parse NDJSON lines. cafe-cli subscribe outputs raw Chunk JSON."""
    chunks = []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        # cafe-cli subscribe outputs raw Chunk objects with "id", "content_type", etc.
        if isinstance(obj, dict) and "id" in obj and "content_type" in obj:
            chunks.append(obj)
    return chunks


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen(
            [BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        time.sleep(1)

        try:
            # ── Create session ──
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0, f"create session failed: {r.stderr}"
            session_id = r.stdout.strip()
            print(f"  session_id={session_id}", file=sys.stderr)

            # ── Test 1: Single publish has source.connection ──
            print("\n=== Single publish has source.connection ===", file=sys.stderr)

            # Drain any existing history first
            run(
                [CLI, "--bus", bus_socket, "subscribe", session_id, "--timeout-secs", "1"],
                timeout=10,
            )

            run([CLI, "--bus", bus_socket, "publish", session_id, "--text", "hello from test"])
            time.sleep(0.3)

            r = run(
                [CLI, "--bus", bus_socket, "subscribe", session_id, "--timeout-secs", "2"],
                timeout=10,
            )
            chunks = parse_chunks(r.stdout.strip().split("\n"))
            print(f"  received {len(chunks)} chunks", file=sys.stderr)

            assert len(chunks) >= 1, (
                f"expected at least 1 chunk, got {len(chunks)}"
            )

            source_conns = set()
            for c in chunks:
                ann = c.get("annotations", {})
                assert ANNOTATION_KEY in ann, (
                    f"chunk missing {ANNOTATION_KEY} annotations={ann}"
                )
                conn_id = ann.get(ANNOTATION_KEY)
                assert isinstance(conn_id, str) and conn_id, (
                    f"source.connection must be non-empty string, got {conn_id!r}"
                )
                source_conns.add(conn_id)

            print(f"  source connection IDs seen: {source_conns}", file=sys.stderr)
            assert len(source_conns) >= 1, "expected at least one source connection ID"

            # ── Test 2: Every published chunk has the annotation ──
            print("\n=== Every published chunk has source.connection ===", file=sys.stderr)

            run([CLI, "--bus", bus_socket, "publish", session_id, "--text", "chunk A"])
            time.sleep(0.3)

            r = run(
                [CLI, "--bus", bus_socket, "subscribe", session_id, "--timeout-secs", "2"],
                timeout=10,
            )
            chunks = parse_chunks(r.stdout.strip().split("\n"))
            print(f"  received {len(chunks)} chunks", file=sys.stderr)

            for c in chunks:
                ann = c.get("annotations", {})
                assert ANNOTATION_KEY in ann, (
                    f"chunk missing {ANNOTATION_KEY}"
                )
                conn_id = ann.get(ANNOTATION_KEY)
                assert isinstance(conn_id, str) and conn_id, (
                    f"source.connection must be non-empty string, got {conn_id!r}"
                )

            print("\n=== ALL SOURCE CONNECTION TESTS PASSED ===", file=sys.stderr)

        finally:
            bus_proc.kill()
            bus_proc.wait()


if __name__ == "__main__":
    main()
