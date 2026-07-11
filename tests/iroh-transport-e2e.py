#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["cryptography"]
# ///
"""
E2E test for iroh transport connectivity to cafe-bus.

Verifies:
1. cafe-bus starts with iroh listener enabled
2. cafe-cli can create sessions over iroh
3. Publish + subscribe works over iroh
4. source.connection annotation present on iroh-published chunks
5. Sessions persist after iroh connection drops
6. Unix socket and iroh subscribers both see the same chunks

Usage:
    cargo build --release
    uv run tests/iroh-transport-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import time

from cryptography.hazmat.primitives.asymmetric import ed25519

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")

ANNOTATION_KEY = "cafe.source.connection"
IROH_RELAY = "https://euc1-1.relay.n0.iroh.link./"


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def generate_key_pair():
    sk = ed25519.Ed25519PrivateKey.generate()
    pk = sk.public_key()
    return sk.private_bytes_raw().hex(), pk.public_bytes_raw().hex()


def cli_unix(socket_path, *args):
    return [CLI, "--bus", socket_path] + list(args)


def cli_iroh(bus_public_hex, *args):
    return [
        CLI,
        "--bus-iroh-key", bus_public_hex,
        "--bus-iroh-relay", IROH_RELAY,
    ] + list(args)


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    bus_secret_hex, bus_public_hex = generate_key_pair()
    print(f"  bus public key: {bus_public_hex}", file=sys.stderr)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["CAFE_BUS_IROH_SECRET_KEY"] = bus_secret_hex

        print("=== Starting cafe-bus (iroh + unix) ===", file=sys.stderr)
        bus_proc = subprocess.Popen(
            [BUS_BIN], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        time.sleep(8)

        try:
            # ── Test 1: iroh create + list ──
            print("\n=== Test 1: Create session over iroh ===", file=sys.stderr)
            r = run(cli_iroh(bus_public_hex, "create-session", "--agent", "default"),
                    env=env, timeout=60)
            assert r.returncode == 0, f"create: {r.stderr}"
            sid_iroh = r.stdout.strip()
            assert sid_iroh, "empty session ID"
            print(f"  sid={sid_iroh}", file=sys.stderr)

            # Verify session persists
            r = run(cli_iroh(bus_public_hex, "list-sessions"), env=env, timeout=30)
            assert r.returncode == 0, f"list: {r.stderr}"
            sessions = json.loads(r.stdout.strip())
            assert any(s["session_id"] == sid_iroh for s in sessions), (
                f"session {sid_iroh} not found in list"
            )
            print("  session persisted ✅", file=sys.stderr)

            # ── Test 2: iroh publish + subscribe ──
            print("\n=== Test 2: Publish + subscribe over iroh ===", file=sys.stderr)
            r = run(cli_iroh(bus_public_hex, "publish", sid_iroh, "--text", "hello via iroh"),
                    env=env, timeout=60)
            assert r.returncode == 0, f"publish: {r.stderr}"
            time.sleep(1.0)

            r = run(cli_iroh(bus_public_hex, "subscribe", sid_iroh, "--timeout-secs", "5"),
                    env=env, timeout=60)
            assert r.returncode == 0, f"subscribe: {r.stderr}"

            # Parse chunks - subscribe outputs raw Chunk JSON
            chunks = []
            for line in r.stdout.strip().split("\n"):
                line = line.strip()
                if not line or line.startswith("subscribed") or line.startswith("history_complete"):
                    continue
                try:
                    obj = json.loads(line)
                    if isinstance(obj, dict) and "id" in obj:
                        chunks.append(obj)
                except json.JSONDecodeError:
                    continue

            assert len(chunks) >= 1, f"expected ≥1 chunk, got {len(chunks)}"
            assert any("hello via iroh" in (c.get("content") or "") for c in chunks), (
                "chunk content not found"
            )
            print(f"  {len(chunks)} chunks received ✅", file=sys.stderr)

            # ── Test 3: source.connection on iroh chunks ──
            print("\n=== Test 3: source.connection over iroh ===", file=sys.stderr)
            for c in chunks:
                ann = c.get("annotations", {})
                assert ANNOTATION_KEY in ann, f"missing {ANNOTATION_KEY}"
                conn_id = ann[ANNOTATION_KEY]
                assert isinstance(conn_id, str) and conn_id, f"bad conn_id: {conn_id!r}"
            print("  source.connection present on all chunks ✅", file=sys.stderr)

            # ── Test 4: Cross-transport — Unix sub sees iroh chunks ──
            print("\n=== Test 4: Unix subscriber sees iroh chunks ===", file=sys.stderr)
            r = run(cli_unix(bus_socket, "subscribe", sid_iroh, "--timeout-secs", "5"),
                    env=env, timeout=10)
            assert r.returncode == 0
            assert "hello via iroh" in r.stdout
            print("  unix subscriber sees iroh-published chunks ✅", file=sys.stderr)

            # ── Test 5: Unix session visible over iroh, no cross-session leaks ──
            print("\n=== Test 5: iroh access to unix session + session isolation ===", file=sys.stderr)
            r = run(cli_unix(bus_socket, "create-session", "--agent", "default"),
                    env=env, timeout=10)
            assert r.returncode == 0
            sid_unix = r.stdout.strip()
            run(cli_unix(bus_socket, "publish", sid_unix, "--text", "only in unix"), env=env)

            # Subscribe to unix session via iroh
            r = run(cli_iroh(bus_public_hex, "subscribe", sid_unix, "--timeout-secs", "5"),
                    env=env, timeout=30)
            assert r.returncode == 0, f"iroh sub to unix session: {r.stderr}"
            # Subscribe outputs NDJSON: subscribed line, chunks, history_complete
            unix_chunk_found = any(
                "only in unix" in line and '"content_type"' in line
                for line in r.stdout.split("\n")
            )
            iroh_leak = any(
                "hello via iroh" in line and '"content_type"' in line
                for line in r.stdout.split("\n")
            )
            assert unix_chunk_found, f"iroh sub should see unix chunk. output: {r.stdout[:500]}"
            assert not iroh_leak, f"iroh sub leaked chunk from iroh session"
            print("  iroh subscriber sees unix chunks, no cross-session leaks ✅", file=sys.stderr)

            print("\n=== ALL IROH TRANSPORT E2E TESTS PASSED ===", file=sys.stderr)

        finally:
            bus_proc.kill()
            bus_proc.wait()


if __name__ == "__main__":
    main()
