#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
SubscribeFiltered + session isolation e2e test.

Tests:
1. SubscribeFiltered with content_type filter only matches specified types
2. Other chunk types are filtered out
3. Session isolation: chunks published to one session don't appear
   in another session's stream

Usage:
    cargo build --release
    uv run tests/bus-filters-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import threading

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
STORE_BIN = os.path.join(RELEASE_DIR, "cafe-store")


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        db_path = os.path.join(tmpdir, "cafe.db")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        try:
            # Create two sessions
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            sid_a = r.stdout.strip()

            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            sid_b = r.stdout.strip()

            print(f"  session_a={sid_a}", file=sys.stderr)
            print(f"  session_b={sid_b}", file=sys.stderr)

            # ── Test 1: SubscribeFiltered with content_type ──
            print("\n=== SubscribeFiltered (content_type=text) ===", file=sys.stderr)

            # Subscribe-all filtered for only text chunks, 3s timeout
            r = run(
                [CLI, "--bus", bus_socket, "subscribe-all", "--content-type", "text", "--timeout-secs", "3"],
                timeout=10,
            )
            sub_lines = r.stdout.strip().split("\n")
            # The filtered subscription should receive no chunks yet
            # (nothing published that matches text)

            # Publish a text chunk and a null chunk
            run([CLI, "--bus", bus_socket, "publish", sid_a, "--text", "hello from text"])

            # Publish a BinaryRef (not text) — should be filtered
            run([CLI, "--bus", bus_socket, "publish", sid_a, "--binary-ref", "--mime", "audio/wav", "--transient"])

            # Now subscribe filtered again — should only get the text chunk
            r = run(
                [CLI, "--bus", bus_socket, "subscribe-all", "--content-type", "text", "--timeout-secs", "3"],
                timeout=10,
            )
            text_chunks = [json.loads(l) for l in r.stdout.strip().split("\n") if l.strip()]
            # Filter for actual chunk events (not SessionCreated etc)
            actual_chunks = [c for c in text_chunks if c.get("event") == "chunk"]
            assert len(actual_chunks) >= 1, f"expected at least 1 text chunk, got {len(actual_chunks)}"
            for c in actual_chunks:
                ct = c.get("chunk", {}).get("content_type", "")
                assert ct in ("text", None), f"got non-text chunk type: {ct}"

            # Now publish a non-transient BinaryRef and text, then subscribe unfiltered
            run([CLI, "--bus", bus_socket, "publish", sid_a, "--binary-ref", "--mime", "image/png"])
            run([CLI, "--bus", bus_socket, "publish", sid_a, "--text", "another text"])

            r = run(
                [CLI, "--bus", bus_socket, "subscribe-all", "--timeout-secs", "3"],
                timeout=10,
            )
            all_chunks = [json.loads(l) for l in r.stdout.strip().split("\n") if l.strip()]
            chunk_types = set()
            for c in all_chunks:
                if c.get("event") == "chunk":
                    ct = c.get("chunk", {}).get("content_type")
                    if ct:
                        chunk_types.add(ct)
            print(f"  filtered (text): {len(actual_chunks)} chunks", file=sys.stderr)
            print(f"  unfiltered types: {chunk_types}", file=sys.stderr)

            assert "binary_ref" in chunk_types, f"binary_ref should appear in unfiltered, got {chunk_types}"
            assert "text" in chunk_types, f"text should appear in unfiltered, got {chunk_types}"

            # ── Test 2: Session isolation ──
            print("\n=== Session isolation ===", file=sys.stderr)

            # Publish to session A
            run([CLI, "--bus", bus_socket, "publish", sid_a, "--text", "only in A"])

            # Publish to session B
            run([CLI, "--bus", bus_socket, "publish", sid_b, "--text", "only in B"])

            # Subscribe to session A only — should not see B's chunks
            r = run(
                [CLI, "--bus", bus_socket, "subscribe", sid_a, "--timeout-secs", "3"],
                timeout=10,
            )
            a_chunks = [json.loads(l) for l in r.stdout.strip().split("\n") if l.strip()]
            a_texts = [c.get("content", "") for c in a_chunks if c.get("content")]
            assert "only in A" in " ".join(a_texts), "session A missing its own chunk"
            assert "only in B" not in " ".join(a_texts), "session A leaked chunk from B"
            print(f"  session A: {len(a_chunks)} chunks, no B leaks ✅", file=sys.stderr)

            # Subscribe to session B only
            r = run(
                [CLI, "--bus", bus_socket, "subscribe", sid_b, "--timeout-secs", "3"],
                timeout=10,
            )
            b_chunks = [json.loads(l) for l in r.stdout.strip().split("\n") if l.strip()]
            b_texts = [c.get("content", "") for c in b_chunks if c.get("content")]
            assert "only in B" in " ".join(b_texts), "session B missing its own chunk"
            assert "only in A" not in " ".join(b_texts), "session B leaked chunk from A"
            print(f"  session B: {len(b_chunks)} chunks, no A leaks ✅", file=sys.stderr)

            print("\n=== ALL BUS FILTER TESTS PASSED ===", file=sys.stderr)

        finally:
            bus_proc.kill()
            bus_proc.wait()


if __name__ == "__main__":
    main()
