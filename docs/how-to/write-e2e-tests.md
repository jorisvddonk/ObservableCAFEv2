# How to: Write and run E2E tests

This guide shows you how to write end-to-end tests for ObservableCAFE and run the
existing suite. It also explains the hard rules this repo enforces around test
reliability.

---

## Philosophy: zero tolerance for silent failures

Every E2E test must **hard-fail** on any unexpected condition. There are no
graceful fallbacks, no conditional passes, no `⚠️` warnings in place of asserts.
Every phase of a multi-step flow is verified with a hard assertion, and
infrastructure failures (a missing model, a down service) are test failures.

A passing test prints exactly `=== ALL ... TESTS PASSED ===`; anything else
means it crashed or asserted.

## Running the existing tests

The Python tests live in `tests/*.py`. Each starts its **own temporary bus** for
isolation — no shared state with the running stack.

```sh
cargo build --release

uv run tests/bus-filters-e2e.py
uv run tests/lifecycle-e2e.py
uv run tests/mcp-bridge-e2e.py
uv run tests/stt-e2e.py
uv run tests/tool-calling-e2e.py
# ... see the README for the full list
```

Requires `uv` (https://docs.astral.sh/uv/). The tests use `#!/usr/bin/env -S uv run`
with inline metadata, so they can be run directly too:

```sh
./tests/bus-filters-e2e.py
```

## Anatomy of a test

Here's the skeleton every test follows (from `tests/bus-filters-e2e.py`):

```python
#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_DIR = os.path.join(PROJECT_ROOT, "target", "release")
CLI = os.path.join(RELEASE_DIR, "cafe-cli")
BUS_BIN = os.path.join(RELEASE_DIR, "cafe-bus")

def main():
    # 1. Guard: release binaries must exist
    for name, path in [("cafe-bus", BUS_BIN), ("cafe-cli", CLI)]:
        if not os.path.exists(path):
            print(f"Build {name} first: cargo build --release", file=sys.stderr)
            sys.exit(1)

    # 2. Isolate: temp dir, temp socket, temp DB
    with tempfile.TemporaryDirectory() as tmpdir:
        bus_socket = os.path.join(tmpdir, "cafe-bus.sock")
        env = os.environ.copy()
        env["CAFE_BUS_SOCKET"] = bus_socket

        # 3. Start the minimal bus
        bus_proc = subprocess.Popen([BUS_BIN], env=env, ...)
        time.sleep(1)
        try:
            # 4. Exercise the system with the CLI, asserting every step
            r = run([CLI, "--bus", bus_socket, "create-session", "--agent", "default"])
            assert r.returncode == 0
            ...
            print("=== ALL BUS FILTER TESTS PASSED ===", file=sys.stderr)
        finally:
            bus_proc.kill()
            bus_proc.wait()

if __name__ == "__main__":
    main()
```

The patterns to copy:

1. **Guard on release builds** — check the binaries exist, exit non-zero with a
   clear message if not.
2. **Isolate with a temp dir** — never touch the running stack or a fixed socket.
3. **Assert every phase** — publish → assert publish; subscribe → assert the
   expected chunks arrived and nothing unexpected did; clean up → assert.
4. **Always clean up in `finally`** — kill the child processes.
5. **Success marker** — print `=== ALL ... TESTS PASSED ===` only after all
   assertions hold.

## Adding a test to CI

When you add a new script to `tests/`, add it to the **`Run E2E bus tests`** step
in `.github/workflows/ci.yml`. CI runs `cargo build --release --workspace`,
`cargo test --release --workspace`, then each E2E script. If your test needs a
live model or external service, make sure CI has it or document the requirement
in the test's docstring.

## Unit tests (Rust)

Unit tests live inline in `src/` under `#[cfg(test)]` modules — there are no
dedicated test files or integration-test directories. Run them with:

```sh
cargo test --workspace
```

## Pitfalls

- **Don't read the success marker** — assert on data, not on the final print.
- **Don't sleep-flake** — if ordering matters, wait/poll for an expected state
  rather than a fixed delay, or increase timeouts with a clear assertion.
- **Never pass on degraded results** — "API unavailable, moving on" is a failure.
- **Verify tests actually fail** — when fixing a bug, confirm the test fails
  before your fix and passes after; a test that passes trivially proves nothing.

---

## Reference

- [README: E2E tests](../../README.md#e2e-tests) — the full runnable list
- [Bus protocol spec](../spec-bus-protocol.md) — the messages the CLI wraps
- [How to: use the cafe-cli](use-the-cli.md) — CLI commands used inside tests
