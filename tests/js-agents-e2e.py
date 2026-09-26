#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
End-to-end test: pure-JS agents via cafe-agent-js (rquickjs, QuickJS-NG).

Covers, each with hard assertions (zero tolerance for silent failures):
 1. Registry: agent-js boots and loads all repo agents-js/*.js files.
 2. Stateless file-loaded agent: user msg -> JS -> rot13.invoke RPC
    (call_id-correlated) -> assistant reply from the JS host.
 3. Stateful agent: JS-local counter accumulates across events in one
    runtime (a stateless runtime would answer 1 every time).
 4. Background agent: session auto-created at boot + initial_config seeded
    as a runtime null chunk.
 5. Tick handling: manual flow tick -> config-driven reply.
 6. Cron: scheduled agent fires on its own.
 7. Hot-reload (stateless): rewriting a fixture file changes behavior
    without restart.
 8. Hot-reload (stateful): rewriting a fixture respawns the runtime with
    fresh JS state (counter resets under the new code marker).
 9. Tool calling (mock LLM): scripted LLM emits a dice.roll marker ->
    JS cafe.tool executes it with bus-visible result -> scripted LLM
    answers; mock log proves initial_config reached the model.

Usage:
    cargo build --release -p cafe-bus -p cafe-cli -p cafe-rot13 -p cafe-agent-js -p cafe-llm -p cafe-dice
    uv run tests/js-agents-e2e.py
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")
ROT13_BIN = os.path.join(RELEASE_DIR, "cafe-rot13")
AGENTJS_BIN = os.path.join(RELEASE_DIR, "cafe-agent-js")
LLM_BIN = os.path.join(RELEASE_DIR, "cafe-llm")
DICE_BIN = os.path.join(RELEASE_DIR, "cafe-dice")
MOCK_LLM = os.path.join(PROJECT_ROOT, "tests", "mock-llm-server.py")

MOCK_PORT = 47981

# Scripted LLM turns: first emits the dice.roll marker, second answers
# after the tool result lands in history.
LLM_SCRIPT = {
    "replies": [
        'Rolling now.<|tool_call|>{"name":"dice.roll","parameters":{"count":2,"sides":6}}<|tool_call_end|>',
        "Done - see the tool result above for your total.",
    ]
}

JS_HOST = "com.nominal.cafe-agent-js"

# Temp-dir fixtures (rewritten mid-test; repo agents-js/*.js stay untouched).
RELOADME_V1 = """\
const manifest = {
  name: "reloadme",
  description: "hot-reload fixture (stateless)",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.publishText("v1:" + event.text);
    }
  }
  return "reloadme v1 done";
}
"""

RELOADME_V2 = RELOADME_V1.replace('"v1:" + event.text', '"v2:" + event.text')

COUNTER2_V1 = """\
const manifest = {
  name: "counter2",
  description: "hot-reload fixture (stateful)",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateful",
};

async function main(cafe) {
  let n = 0;
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      n += 1;
      await cafe.publishText("v1:count=" + n);
    }
  }
  return "counter2 v1 done";
}
"""

COUNTER2_V2 = COUNTER2_V1.replace('"v1:count=" + n', '"v2:count=" + n')

FETCH_PORT = 48211
FETCH_URL = f"http://127.0.0.1:{FETCH_PORT}/doc.txt"

FETCHER = """\
const manifest = {
  name: "fetcher",
  description: "fetches a URL and publishes it with its annotations",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type !== "user_message") continue;
    const res = await cafe.fetch("%s");
    await res.publish();
  }
  return "fetcher: done";
}
""" % FETCH_URL


class _FetchHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        body = b"fetched-body"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


ANNOTATOR = """\
const manifest = {
  name: "annotator",
  description: "publishes chunks with custom annotations",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.publish({
        type: "text",
        content: "annotated-reply",
        annotations: { "test.kind": "annotation", "test.count": 3 },
      });
      await cafe.annotate({ "test.signal": "annotated-signal", "test.null": true });
    }
  }
  return "annotator: done";
}
"""


def run(cmd, **kwargs):
    print(f"  + {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


def history(cli, sock, session):
    r = run([cli, "--bus", sock, "history", session])
    assert r.returncode == 0, f"history {session} failed: {r.stderr}"
    chunks = []
    for line in r.stdout.strip().split("\n"):
        if line.strip():
            chunks.append(json.loads(line))
    return chunks


def wait_for(cli, sock, session, pred, timeout, desc):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for c in history(cli, sock, session):
            if pred(c):
                return c
        time.sleep(1)
    raise AssertionError(f"TIMEOUT after {timeout}s waiting for {desc} in session {session}")


def host_text(content):
    def pred(c):
        return (c.get("content_type") == "text"
                and c.get("producer") == JS_HOST
                and c.get("annotations", {}).get("chat.role") == "assistant"
                and c.get("content") == content)
    return pred


def main():
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI),
                       ("cafe-rot13", ROT13_BIN), ("cafe-agent-js", AGENTJS_BIN),
                       ("cafe-llm", LLM_BIN), ("cafe-dice", DICE_BIN)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release -p {name}", file=sys.stderr)
            sys.exit(1)

    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        fixtures = os.path.join(tmpdir, "agents-js")
        os.mkdir(fixtures)
        with open(os.path.join(fixtures, "reloadme.js"), "w") as f:
            f.write(RELOADME_V1)
        with open(os.path.join(fixtures, "counter2.js"), "w") as f:
            f.write(COUNTER2_V1)
        with open(os.path.join(fixtures, "annotator.js"), "w") as f:
            f.write(ANNOTATOR)
        with open(os.path.join(fixtures, "fetcher.js"), "w") as f:
            f.write(FETCHER)
        agent_log = os.path.join(tmpdir, "agent-js.log")

        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket
        env["CAFE_JS_AGENT_PATHS"] = fixtures

        # Local HTTP server for the cafe.fetch annotation phase.
        fetch_srv = HTTPServer(("127.0.0.1", FETCH_PORT), _FetchHandler)
        threading.Thread(target=fetch_srv.serve_forever, daemon=True).start()

        print("=== Starting cafe-bus ===", file=sys.stderr)
        bus_proc = subprocess.Popen([BUS_BIN], env=env,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.time() + 15
        while not os.path.exists(bus_socket) and time.time() < deadline:
            assert bus_proc.poll() is None, "cafe-bus exited before creating its socket"
            time.sleep(0.2)
        assert os.path.exists(bus_socket), "bus socket never appeared"

        print("=== Starting cafe-rot13 ===", file=sys.stderr)
        rot13_proc = subprocess.Popen([ROT13_BIN], env=env,
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1)

        print("=== Starting cafe-agent-js ===", file=sys.stderr)
        with open(agent_log, "w") as logf:
            agent_proc = subprocess.Popen([AGENTJS_BIN], env=env, cwd=PROJECT_ROOT,
                                          stdout=logf, stderr=subprocess.STDOUT)
            time.sleep(3)
            mock_proc = llm_proc = dice_proc = None

        try:
            with open(agent_log) as f:
                log = f.read()
            # 17 repo agents (demo, heartbeat, ticker, counter, dice-llm,
            # default, rot13, stt, fetch, knowledgebase, voice, volition,
            # comfy, dice, epub-narrator, sheetbot, rss-summarizer) + 4
            # fixtures (reloadme, counter2, annotator, fetcher). Count is
            # asserted exactly so an agent silently failing to load fails.
            assert "loaded 21 JS agents" in log, f"registry did not load 21 agents:\n{log[-3000:]}"
            print("  registry loaded 21 JS agents", file=sys.stderr)

            # --- 2. stateless file-loaded agent (repo demo.js) ---
            print("=== Stateless round-trip (demo) ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "demo"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            demo = r.stdout.strip()
            assert demo, "empty session id"
            time.sleep(2)  # let the host attach before publishing
            r = run([CLI, "--bus", bus_socket, "publish", demo, "--text", "hello world"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"

            def rot13_reply(c):
                return (c.get("content_type") == "text"
                        and c.get("producer") == "com.nominal.cafe-rot13"
                        and c.get("annotations", {}).get("chat.role") == "assistant"
                        and c.get("content") == "uryyb jbeyq")
            wait_for(CLI, bus_socket, demo, rot13_reply, 25, "rot13 reply")
            # Exactly one reply chunk: the agent must not republish an
            # evaluator's output (that was a duplicate-reply bug).
            replies = [c for c in history(CLI, bus_socket, demo)
                       if c.get("content_type") == "text"
                       and c.get("annotations", {}).get("chat.role") == "assistant"]
            assert len(replies) == 1, \
                f"expected exactly 1 assistant reply, saw {len(replies)}"
            # The RPC request/response pair is correlated by call_id.
            reqs = [x for x in history(CLI, bus_socket, demo)
                    if x.get("annotations", {}).get("cafe.jsonrpc.request", {})
                    .get("method") == "rot13.invoke"]
            assert len(reqs) == 1, f"expected 1 rot13.invoke request, saw {len(reqs)}"
            call_id = reqs[0]["annotations"]["cafe.jsonrpc.request"]["id"]
            resps = [x for x in history(CLI, bus_socket, demo)
                     if x.get("annotations", {}).get("cafe.jsonrpc.response", {})
                     .get("id") == call_id]
            assert len(resps) == 1, "no response matching the request call_id"
            assert resps[0]["annotations"]["cafe.jsonrpc.response"]["result"] == \
                {"text": "uryyb jbeyq"}
            print("  stateless RPC round-trip with call_id correlation", file=sys.stderr)

            # --- 2b. agent-published chunks carry custom annotations ---
            print("=== Annotations (cafe.publish / cafe.annotate) ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "annotator"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            annotator = r.stdout.strip()
            assert annotator, "empty session id"
            time.sleep(2)
            r = run([CLI, "--bus", bus_socket, "publish", annotator, "--text", "go"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"

            def annotated(c):
                ann = c.get("annotations", {})
                return (c.get("producer") == JS_HOST
                        and c.get("content") == "annotated-reply"
                        and ann.get("test.kind") == "annotation"
                        and ann.get("test.count") == 3)
            c = wait_for(CLI, bus_socket, annotator, annotated, 25,
                         "annotated text chunk")
            assert c["annotations"]["chat.role"] == "assistant", c["annotations"]

            def signal(c):
                ann = c.get("annotations", {})
                return (c.get("producer") == JS_HOST
                        and c.get("content_type") == "null"
                        and ann.get("test.signal") == "annotated-signal"
                        and ann.get("test.null") is True)
            s = wait_for(CLI, bus_socket, annotator, signal, 25,
                         "annotated null chunk")
            assert "chat.role" not in s["annotations"], s["annotations"]
            print("  text + null chunks carried custom annotations", file=sys.stderr)

            # --- 2c. cafe.fetch annotates fetched content as untrusted ---
            print("=== Fetch annotations ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "fetcher"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            fetcher = r.stdout.strip()
            assert fetcher, "empty session id"
            time.sleep(2)
            r = run([CLI, "--bus", bus_socket, "publish", fetcher, "--text", "go"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"

            def fetched(c):
                ann = c.get("annotations", {})
                return (c.get("producer") == JS_HOST
                        and c.get("content") == "fetched-body"
                        and ann.get("web.source_url") == FETCH_URL
                        and ann.get("web.content_type") == "text/plain"
                        and isinstance(ann.get("web.fetch_time"), int)
                        and ann.get("security.trust-level", {}).get("trusted") is False
                        and ann.get("security.trust-level", {}).get("source") == "web")
            wait_for(CLI, bus_socket, fetcher, fetched, 25,
                     "fetched chunk with web/security annotations")
            print("  fetched chunk carries web.* + untrusted security.trust-level",
                  file=sys.stderr)

            # --- 3. stateful counter accumulates JS-local state ---
            print("=== Stateful counter ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "counter"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            counter = r.stdout.strip()
            time.sleep(2)
            r = run([CLI, "--bus", bus_socket, "publish", counter, "--text", "a"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            wait_for(CLI, bus_socket, counter, host_text("count=1"), 25, "count=1")
            r = run([CLI, "--bus", bus_socket, "publish", counter, "--text", "b"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            wait_for(CLI, bus_socket, counter, host_text("count=2"), 25,
                     "count=2 (stateless would answer 1 again)")
            print("  counter reached count=2 in one runtime", file=sys.stderr)

            # --- 4. background session + config seeding ---
            print("=== Background heartbeat ===", file=sys.stderr)
            hb = history(CLI, bus_socket, "heartbeat")
            assert hb, "heartbeat background session was not auto-created"
            cfg = [c for c in hb
                   if c.get("annotations", {}).get("config.type") == "runtime"]
            assert cfg, "no runtime config chunk seeded in heartbeat history"
            assert cfg[0]["annotations"].get("config.js.agent") == "heartbeat", \
                f"initial_config not seeded: {cfg[0]['annotations']}"
            print("  background session + config seeding", file=sys.stderr)

            # --- 5. tick handling via cafe.config() ---
            r = run([CLI, "--bus", bus_socket, "publish", "heartbeat", "--text", "ping"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            # The user-message reply comes from cafe-rot13 (the agent only
            # invokes it; it must not republish).
            wait_for(CLI, bus_socket, "heartbeat",
                     lambda c: c.get("content_type") == "text"
                     and c.get("producer") == "com.nominal.cafe-rot13"
                     and c.get("content") == "cvat", 25, "heartbeat rot13 reply")
            r = run([CLI, "--bus", bus_socket, "publish", "heartbeat", "--null",
                     "--annotation", "cafe.flow.signal=tick"])
            assert r.returncode == 0, f"tick publish failed: {r.stderr}"
            wait_for(CLI, bus_socket, "heartbeat",
                     host_text("heartbeat tick acknowledged"), 25, "tick reply")
            print("  user msg + manual tick answered", file=sys.stderr)

            # --- 6. cron fires on its own ---
            print("=== Cron ticker (10s) ===", file=sys.stderr)
            wait_for(CLI, bus_socket, "ticker", host_text("ticker tick"), 45, "cron tick")
            print("  scheduled tick observed", file=sys.stderr)

            # --- 7. hot-reload, stateless: behavior changes without restart ---
            print("=== Hot-reload stateless ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "reloadme"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            reloadme = r.stdout.strip()
            time.sleep(2)
            r = run([CLI, "--bus", bus_socket, "publish", reloadme, "--text", "one"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            wait_for(CLI, bus_socket, reloadme, host_text("v1:one"), 25, "v1 reply")
            with open(os.path.join(fixtures, "reloadme.js"), "w") as f:
                f.write(RELOADME_V2)
            deadline = time.time() + 10
            reloaded = False
            while time.time() < deadline:
                with open(agent_log) as f:
                    if "hot-reloaded agent 'reloadme'" in f.read():
                        reloaded = True
                        break
                time.sleep(0.5)
            assert reloaded, "hot-reload log line never appeared for reloadme.js"
            r = run([CLI, "--bus", bus_socket, "publish", reloadme, "--text", "two"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            wait_for(CLI, bus_socket, reloadme, host_text("v2:two"), 25,
                     "v2 reply on the SAME session (no re-attach)")
            print("  stateless hot-reload changed behavior in place", file=sys.stderr)

            # --- 8. hot-reload, stateful: respawn with fresh JS state ---
            print("=== Hot-reload stateful ===", file=sys.stderr)
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "counter2"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            counter2 = r.stdout.strip()
            time.sleep(2)
            r = run([CLI, "--bus", bus_socket, "publish", counter2, "--text", "a"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            wait_for(CLI, bus_socket, counter2, host_text("v1:count=1"), 25, "v1 count=1")
            with open(os.path.join(fixtures, "counter2.js"), "w") as f:
                f.write(COUNTER2_V2)
            time.sleep(3)  # let the watcher reload the registry
            r = run([CLI, "--bus", bus_socket, "publish", counter2, "--text", "b"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"
            # Respawned runtime: new code marker (v2) AND reset counter (1,
            # not 2) — continuation would answer v1:count=2.
            wait_for(CLI, bus_socket, counter2, host_text("v2:count=1"), 25,
                     "v2:count=1 (respawned with fresh state)")
            print("  stateful hot-reload respawned with fresh state", file=sys.stderr)

            # --- 9. tool calling against a scripted mock LLM ---
            print("=== Tool calling (mock LLM) ===", file=sys.stderr)
            script_path = os.path.join(tmpdir, "llm-script.json")
            with open(script_path, "w") as f:
                json.dump(LLM_SCRIPT, f)
            mock_log = os.path.join(tmpdir, "mock-llm.log")
            mock_proc = subprocess.Popen(
                [sys.executable, MOCK_LLM, "--port", str(MOCK_PORT),
                 "--log", mock_log, "--script", script_path],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            deadline = time.time() + 15
            import socket as _socket
            while time.time() < deadline:
                try:
                    _socket.create_connection(("127.0.0.1", MOCK_PORT), timeout=1).close()
                    break
                except OSError:
                    assert mock_proc.poll() is None, "mock LLM exited before listening"
                    time.sleep(0.2)
            else:
                raise AssertionError("mock LLM never listened")
            llm_env = env.copy()
            llm_env["LLM_BACKEND"] = "openai"
            llm_env["OPENAI_URL"] = f"http://localhost:{MOCK_PORT}/v1"
            llm_env["OPENAI_MODEL"] = "mock-model"
            llm_proc = subprocess.Popen([LLM_BIN], env=llm_env,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            dice_proc = subprocess.Popen([DICE_BIN], env=env,
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(3)

            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "dice-llm"])
            assert r.returncode == 0, f"create-session failed: {r.stderr}"
            dicellm = r.stdout.strip()
            time.sleep(2)
            r = run([CLI, "--bus", bus_socket, "publish", dicellm, "--text", "roll 2d6"])
            assert r.returncode == 0, f"publish failed: {r.stderr}"

            # The tool executed with a bus-visible result in range for 2d6.
            def tool_result(c):
                res = c.get("annotations", {}).get("cafe.tool.result", {})
                return (res.get("name") == "dice.roll"
                        and isinstance(res.get("output", {}).get("result"), int))
            c = wait_for(CLI, bus_socket, dicellm, tool_result, 40, "dice.tool.result")
            total = c["annotations"]["cafe.tool.result"]["output"]["result"]
            assert 2 <= total <= 12, f"2d6 total out of range: {total}"
            print(f"  dice.roll result={total}", file=sys.stderr)
            # The scripted second turn answered after reading that result.
            wait_for(CLI, bus_socket, dicellm,
                     lambda c: c.get("content_type") == "text"
                     and c.get("annotations", {}).get("chat.role") == "assistant"
                     and "Done - see the tool result" in (c.get("content") or ""), 40,
                     "scripted final answer")
            # The mock saw both turns, with our seeded system prompt.
            with open(mock_log) as f:
                mock_reqs = [json.loads(l) for l in f if l.strip()]
            assert len(mock_reqs) == 2, f"expected 2 LLM requests, saw {len(mock_reqs)}"
            assert "dice-rolling" in mock_reqs[0]["system_prompt"], \
                "initial_config system prompt never reached the model"
            # Turn 2 must carry the tool result in its messages: cafe.tool
            # publishes the readable chunk a follow-up LLM turn reads.
            turn2 = json.dumps(mock_reqs[1].get("messages", []))
            assert "dice.roll" in turn2 and "Tool call completed" in turn2, \
                "tool result never reached the follow-up LLM turn"
            print("  mock LLM saw 2 turns with seeded system prompt", file=sys.stderr)

            # --- cleanup ---
            for s in [demo, annotator, fetcher, counter, reloadme, counter2, dicellm]:
                r = run([CLI, "--bus", bus_socket, "delete-session", s])
                assert r.returncode == 0, f"delete-session {s} failed: {r.stderr}"

        finally:
            fetch_srv.shutdown()
            for p in [bus_proc, rot13_proc, agent_proc, mock_proc, llm_proc, dice_proc]:
                if p is not None:
                    p.kill()
            for p in [bus_proc, rot13_proc, agent_proc, mock_proc, llm_proc, dice_proc]:
                if p is not None:
                    p.wait()

    print("=== ALL JS AGENT TESTS PASSED ===")


if __name__ == "__main__":
    main()
