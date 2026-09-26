#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: evaluator schema announcement and session propagation.

Tests:
1. cafe-rot13 announces its schema on the __schema__ session
2. A subscriber can discover the schema by subscribing to __schema__
3. The schema chunk contains correct name, description, config_schema, rpc_params_schema
4. cafe-agent-runtime discovers the schema and propagates it into agent sessions
5. A subscriber can find evaluator schemas in the agent's session history

Phase 2 uses a TOML fixture agent owned by cafe-agent-runtime (JS agents
shadow same-named TOML agents, so a real name like `rot13` would be ignored
by the runtime).

Usage:
    cargo build --release
    uv run tests/schema-announcement-e2e.py
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
import time

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
ROT13_BIN = os.path.join(RELEASE_DIR, "cafe-rot13")
AGENT_RUNTIME_BIN = os.path.join(RELEASE_DIR, "cafe-agent-runtime")

TIMEOUT_SECS = 30


class BusConnection:
    """A raw Unix socket connection to cafe-bus with JSON-line framing."""

    def __init__(self, socket_path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(socket_path)
        self.buf = b""
        self.send_msg({"op": "set_meta", "role": None})
        msg = self.read_msg()
        assert msg.get("event") in ("connected", "codec_set"), f"expected Connected/CodecSet, got {msg}"

    def send_msg(self, msg: dict):
        data = json.dumps(msg, separators=(",", ":")) + "\n"
        self.sock.sendall(data.encode())

    def read_msg(self, timeout: float | None = TIMEOUT_SECS) -> dict:
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


def verify_rot13_schema(schema: dict, label: str):
    """Assert the given schema dict is a valid rot13 evaluator schema."""
    assert schema["name"] == "rot13", f"[{label}] expected name 'rot13', got '{schema['name']}'"
    assert "ROT13" in schema.get("description", ""), f"[{label}] description missing 'ROT13': {schema['description']}"
    assert schema["config_schema"]["type"] == "object", f"[{label}] config_schema should be an object schema"
    assert schema["rpc_params_schema"]["type"] == "object", f"[{label}] rpc_params_schema should be an object schema"
    assert "text" in schema["rpc_params_schema"].get("properties", {}), f"[{label}] rpc_params should have 'text' property"
    assert schema["rpc_params_schema"]["properties"]["text"]["type"] == "string", f"[{label}] 'text' should be type string"


def subscribe_and_collect(conn: BusConnection, session_id: str, annotation_key: str) -> list:
    """Subscribe to a session and collect all chunks with the given annotation key."""
    conn.send_msg({"op": "subscribe", "session_id": session_id})
    results = []
    while True:
        msg = conn.read_msg()
        event = msg.get("event")
        if event == "history_complete":
            break
        if event == "chunk":
            annotations = msg.get("chunk", {}).get("annotations", {})
            if annotation_key in annotations:
                results.append(annotations[annotation_key])
    return results


def dump_logs(tmpdir: str, names: list[str], lines: int = 30):
    for name in names:
        log_path = os.path.join(tmpdir, f"{name}.log")
        if os.path.exists(log_path):
            with open(log_path) as f:
                print(f"  {name} log:", file=sys.stderr)
                for line in f.readlines()[-lines:]:
                    print(f"    {line.rstrip()}", file=sys.stderr)


def main():
    missing = []
    for label, path in [("cafe-bus", BUS_BIN), ("cafe-rot13", ROT13_BIN), ("cafe-agent-runtime", AGENT_RUNTIME_BIN)]:
        if not os.path.exists(path):
            missing.append(label)
    if missing:
        print(f"Build first: cargo build --release -p {' -p '.join(missing)}", file=sys.stderr)
        sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")

        # A TOML agent owned by cafe-agent-runtime (unique name so no JS agent
        # shadows it), used to verify in-session schema propagation.
        agents_dir = os.path.join(tmpdir, "agents")
        os.mkdir(agents_dir)
        with open(os.path.join(agents_dir, "schema-e2e.toml"), "w") as f:
            f.write(
                'name = "schema-e2e"\n'
                'description = "schema propagation fixture"\n'
                "background = false\n"
                "allows_reload = true\n"
                "persists_state = false\n\n"
                "[[steps]]\n"
                'id = "rot13"\n'
                'type = "rot13"\n'
                'trigger = "user_message"\n'
            )

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        # The runtime prefers ObservableCAFE_AGENT_SEARCH_PATHS over
        # CAFE_AGENT_PATHS; set both so the fixture is found regardless of the
        # caller's shell. (Also clear CAFE_JS_AGENT_PATHS so JS shadowing only
        # sees ./agents-js.)
        env["ObservableCAFE_AGENT_SEARCH_PATHS"] = agents_dir
        env["CAFE_AGENT_PATHS"] = agents_dir
        env.pop("CAFE_JS_AGENT_PATHS", None)
        env["RUST_LOG"] = "info"

        procs = {}

        def start(name, bin_path, **kwargs):
            log = open(os.path.join(tmpdir, f"{name}.log"), "w", buffering=1)
            kwargs.setdefault("env", env)
            kwargs.setdefault("cwd", PROJECT_ROOT)
            kwargs["stdout"] = log
            kwargs["stderr"] = log
            procs[name] = subprocess.Popen([bin_path], **kwargs)

        # 1. Start cafe-bus
        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_log = open(os.path.join(tmpdir, "bus.log"), "w", buffering=1)
        procs["bus"] = subprocess.Popen([BUS_BIN], env=env, stdout=bus_log, stderr=bus_log)
        time.sleep(2)

        # 2. Start cafe-rot13 (evaluator that announces its schema)
        print("=== Starting cafe-rot13 ===", file=sys.stderr)
        start("rot13", ROT13_BIN)
        time.sleep(2)

        # 3. Start cafe-agent-runtime (discovers schemas from __schema__,
        #    propagates them into agent sessions)
        print("=== Starting cafe-agent-runtime ===", file=sys.stderr)
        start("agent-runtime", AGENT_RUNTIME_BIN)
        time.sleep(3)

        # Check all processes are alive
        for name, p in list(procs.items()):
            if p.poll() is not None:
                print(f"  {name} exited with code {p.returncode}", file=sys.stderr)
                dump_logs(tmpdir, [name])
                sys.exit(1)

        try:
            # ── Phase 1: Verify schema on __schema__ session ──
            print("\n=== Phase 1: Verify schema on __schema__ ===", file=sys.stderr)
            conn1 = BusConnection(bus_socket)
            schema_chunks = subscribe_and_collect(conn1, "__schema__", "cafe.schema.evaluator")
            conn1.close()

            if not schema_chunks:
                print("  FAIL: no schema chunks found in __schema__ session", file=sys.stderr)
                dump_logs(tmpdir, ["rot13", "agent-runtime"])
                sys.exit(1)

            rot13_on_schema = None
            for s in schema_chunks:
                if s.get("name") == "rot13":
                    rot13_on_schema = s
                    break

            if rot13_on_schema is None:
                print(f"  FAIL: no rot13 schema in __schema__ (found: {[s.get('name') for s in schema_chunks]})", file=sys.stderr)
                dump_logs(tmpdir, ["rot13", "agent-runtime"])
                sys.exit(1)

            verify_rot13_schema(rot13_on_schema, "__schema__")
            print(f"  rot13 schema on __schema__: name={rot13_on_schema['name']} ✅", file=sys.stderr)

            # ── Phase 2: Create an agent session and verify schema propagation ──
            print("\n=== Phase 2: Verify schema in agent session ===", file=sys.stderr)

            e2e_session_id = "e2e-schema-test"
            conn2 = BusConnection(bus_socket)

            # Create a session for the fixture agent (so agent-runtime's
            # pipeline subscriber recognises it and publishes schemas)
            conn2.send_msg({
                "op": "create_session",
                "session_id": e2e_session_id,
                "agent_id": "schema-e2e",
                "config": {"ephemeral": {"keepalive_secs": 3600}},
            })
            resp = conn2.read_msg()
            assert resp.get("event") == "session_created", f"expected session_created, got {resp}"
            print(f"  created session {e2e_session_id} for agent 'schema-e2e'", file=sys.stderr)

            # Subscribe and collect schema chunks published by agent-runtime
            # We may need to wait briefly for agent-runtime's pipeline subscriber
            # to see the SessionCreated event and publish schemas.
            time.sleep(1)
            session_schemas = subscribe_and_collect(conn2, e2e_session_id, "cafe.schema.evaluator")
            conn2.close()

            if not session_schemas:
                print("  FAIL: no evaluator schema chunks found in agent session", file=sys.stderr)
                dump_logs(tmpdir, ["agent-runtime", "rot13"])
                sys.exit(1)

            rot13_in_session = None
            for s in session_schemas:
                if s.get("name") == "rot13":
                    rot13_in_session = s
                    break

            if rot13_in_session is None:
                print(f"  FAIL: no rot13 schema in agent session (found: {[s.get('name') for s in session_schemas]})", file=sys.stderr)
                dump_logs(tmpdir, ["agent-runtime", "rot13"])
                sys.exit(1)

            verify_rot13_schema(rot13_in_session, "agent-session")
            print(f"  rot13 schema in agent session: name={rot13_in_session['name']} ✅", file=sys.stderr)

        except Exception as e:
            print(f"  FAIL: {e}", file=sys.stderr)
            dump_logs(tmpdir, ["bus", "rot13", "agent-runtime"])
            sys.exit(1)

        finally:
            for name, p in procs.items():
                p.terminate()
            for name, p in procs.items():
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()

    print("\n=== ALL SCHEMA ANNOUNCEMENT TESTS PASSED ===", file=sys.stderr)


if __name__ == "__main__":
    main()
