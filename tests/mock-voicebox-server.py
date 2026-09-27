#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Mock Voicebox server for hermetic E2E tests.

Serves the subset of the Voicebox API that cafe-tts / cafe-stt use:

    GET  /profiles        -> JSON array of {id, name}
    POST /generate/stream -> 200 audio/wav (a short silent WAV)
    POST /transcribe      -> JSON {text, duration}

Usage:
    uv run tests/mock-voicebox-server.py --port 47941 --log /tmp/mock-voicebox.log
"""

import argparse
import json
import struct
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

# Deterministic transcript returned by /transcribe.
TRANSCRIPT = "the quick brown fox jumps over the lazy dog"

# Profiles resolvable by name (cafe-tts resolves a requested profile name via
# GET /profiles before synthesizing).
PROFILES = [
    {"id": "00000000-0000-0000-0000-000000000001", "name": "Test"},
    {"id": "00000000-0000-0000-0000-000000000002", "name": "default"},
    {"id": "00000000-0000-0000-0000-000000000003", "name": "Volition"},
    {"id": "00000000-0000-0000-0000-000000000004", "name": "voice"},
]


def make_wav(seconds: float = 0.25, rate: int = 22050) -> bytes:
    """A valid 16-bit mono PCM WAV of silence."""
    samples = b"\x00\x00" * int(rate * seconds)
    header = (
        b"RIFF"
        + struct.pack("<I", 36 + len(samples))
        + b"WAVE"
        + b"fmt "
        + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
        + b"data"
        + struct.pack("<I", len(samples))
    )
    return header + samples


WAV = make_wav()


class MockVoiceboxHandler(BaseHTTPRequestHandler):
    log_file = "/tmp/mock-voicebox.log"

    def log_message(self, *args):
        pass

    def _record(self, entry: dict):
        with open(self.log_file, "a") as f:
            f.write(json.dumps(entry) + "\n")

    def _send(self, code: int, content_type: str, body: bytes):
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/profiles":
            self._record({"method": "GET", "path": self.path, "time": time.time()})
            self._send(200, "application/json", json.dumps(PROFILES).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
        if self.path == "/generate/stream":
            self._record(
                {"method": "POST", "path": self.path, "bytes": len(raw), "time": time.time()}
            )
            self._send(200, "audio/wav", WAV)
        elif self.path == "/transcribe":
            self._record(
                {"method": "POST", "path": self.path, "bytes": len(raw), "time": time.time()}
            )
            body = json.dumps({"text": TRANSCRIPT, "duration": 0.5}).encode()
            self._send(200, "application/json", body)
        else:
            self.send_response(404)
            self.end_headers()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=17493)
    parser.add_argument("--log", default="/tmp/mock-voicebox.log")
    args = parser.parse_args()

    MockVoiceboxHandler.log_file = args.log
    server = HTTPServer(("0.0.0.0", args.port), MockVoiceboxHandler)
    print(f"mock-voicebox-server listening on {args.port}", file=sys.stderr, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.shutdown()


if __name__ == "__main__":
    main()
