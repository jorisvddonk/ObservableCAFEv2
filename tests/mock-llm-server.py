#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx"]
# ///
"""
Mock LLM server that logs model name and system prompt.
Returns a canned SSE response so cafe-llm's streaming works.

With --script FILE (JSON {"replies": [...]}), serves the replies in order
(beyond the end, repeats the last) instead of the canned text. This drives
multi-turn flows like tool calling: first reply emits a tool-call marker,
second reply answers after the tool result lands in history.

Usage:
    uv run tests/mock-llm-server.py --port 49995 --log /tmp/mock-llm.log
    uv run tests/mock-llm-server.py --port 49995 --log /tmp/mock-llm.log \
        --script /tmp/script.json
"""

import argparse
import json
import os
import sys
import time
from http.server import HTTPServer, BaseHTTPRequestHandler


class MockLLMHandler(BaseHTTPRequestHandler):
    log_file = "/tmp/mock-llm.log"
    requests = []  # class-level buffer for in-process access
    script_replies = []  # scripted replies; empty = canned behavior
    script_index = 0

    def _write_log(self, data):
        with open(self.log_file, "a") as f:
            f.write(json.dumps(data) + "\n")
        self.requests.append(data)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length).decode()
        req = json.loads(body)

        model = req.get("model", "unknown")
        messages = req.get("messages", [])

        # Extract system prompt
        system_prompt = ""
        for m in messages:
            if m.get("role") == "system":
                system_prompt = m.get("content", "")
                break

        if MockLLMHandler.script_replies:
            idx = min(MockLLMHandler.script_index, len(MockLLMHandler.script_replies) - 1)
            reply = MockLLMHandler.script_replies[idx]
            MockLLMHandler.script_index += 1
        else:
            idx = None
            # Return a canned SSE response
            reply = f"You asked using {model}. System prompt starts with: {system_prompt[:60]}..."

        entry = {
            "model": model,
            "system_prompt": system_prompt,
            "reply_index": idx,
            "timestamp": time.time(),
        }
        self._write_log(entry)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()

        # Stream tokens
        for word in reply.split():
            chunk = {
                "id": "mock-llm-0",
                "object": "chat.completion.chunk",
                "choices": [{"delta": {"content": word + " "}, "index": 0}],
            }
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()

        # Final chunk with stream_complete equivalent
        final = {
            "id": "mock-llm-0",
            "object": "chat.completion.chunk",
            "choices": [{"delta": {}, "finish_reason": "stop", "index": 0}],
        }
        self.wfile.write(f"data: {json.dumps(final)}\n\n".encode())
        self.wfile.write("data: [DONE]\n\n".encode())
        self.wfile.flush()


def run(port, log_file, script=None):
    MockLLMHandler.log_file = log_file
    if script:
        with open(script) as f:
            MockLLMHandler.script_replies = json.load(f)["replies"]
        MockLLMHandler.script_index = 0
    server = HTTPServer(("0.0.0.0", port), MockLLMHandler)
    print(f"mock-llm-server listening on {port}", file=sys.stderr, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.shutdown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=49995)
    parser.add_argument("--log", default="/tmp/mock-llm.log")
    parser.add_argument("--script", default=None,
                        help='JSON file {"replies": [...]} served in order')
    args = parser.parse_args()
    run(args.port, args.log, args.script)
