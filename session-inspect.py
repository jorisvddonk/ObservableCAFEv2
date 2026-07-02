#!/usr/bin/env python3
"""List session IDs or dump raw session contents from cafe-server."""

import argparse
import json
import os
import sys
import urllib.request
import urllib.error


def api_get(url: str, token: str) -> dict:
    req = urllib.request.Request(url)
    req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read().decode("utf-8"))


def list_sessions(base_url: str, token: str) -> None:
    data = api_get(f"{base_url}/api/sessions", token)
    sessions = data if isinstance(data, list) else data.get("sessions", [])
    for s in sessions:
        print(s.get("session_id", s.get("id", "?")))


def dump_session(base_url: str, token: str, session_id: str) -> None:
    data = api_get(f"{base_url}/api/sessions/{session_id}/history", token)
    chunks = data.get("chunks", [])
    for chunk in chunks:
        print(json.dumps(chunk, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser(description="Inspect cafe-server sessions")
    parser.add_argument("--url", default=os.environ.get("CAFE_SERVER_URL", "http://localhost:4000"))
    parser.add_argument("--token", default=os.environ.get("CAFE_TOKEN", ""))
    parser.add_argument("--list", action="store_true", help="List session IDs")
    parser.add_argument("--session", help="Dump raw contents of a session by ID")

    args = parser.parse_args()

    if not args.token:
        print("Error: --token or CAFE_TOKEN required", file=sys.stderr)
        sys.exit(1)

    if args.list:
        list_sessions(args.url, args.token)
    elif args.session:
        dump_session(args.url, args.token, args.session)
    else:
        # Default to listing sessions
        list_sessions(args.url, args.token)


if __name__ == "__main__":
    main()
