# Agents

## ADR conventions

ADRs live in `docs/adr-NNN-title.md`. Each ADR has:
- **Status**: `Accepted`, `Implemented` (+ commit), `Superseded by ADR-NNN`, `Design complete, not yet implemented`
- **Date** (optional, when status changed)
- **Driver** (optional — what forced the decision)
- **Context** — problem being solved
- **Decision** — what was chosen and why
- **Consequences** — tradeoffs, impacts
- **Alternatives considered** (optional)

Use the next available NNN (1xx for new).

## Commit conventions

- Use conventional commits: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:`, etc. Lowercase after prefix. Include crate/area scope when possible (e.g. `feat(cafe-tui):`). May reference ADRs.
- This is a new convention — historical commits don't follow it.

## No local state in code or commits

Never mention local setup — launchd, servers, locally available models, or anything environment-specific — in code, comments, or commit messages. Keep all artifacts machine-independent.

## Language preferences

- **Rust** — primary language for all services, libraries, and binaries.
- **TypeScript** — frontend (`cafe-web`) and SDK (`cafe-web-sdk`).
- **Go** — Telegram bridge (`cafe-telegram`).
- **Python** — E2E tests and tooling only.

## Test locations

- **Python E2E tests:** `tests/*.py` (run against live services)
- **Rust unit tests:** inline in `src/` files under `#[cfg(test)]` modules — no dedicated test files or integration test directories

## CI

GitHub Actions workflow at `.github/workflows/ci.yml` runs on push/PR to `main`:
- `cargo build --release --workspace`
- `cargo test --release --workspace`
- E2E bus tests: `bus-filters-e2e.py`, `source-connection-e2e.py`, `ephemeral-sessions-e2e.py`, `tts-e2e.py`

**When adding a new E2E test script to `tests/`, add it to the CI workflow's `Run E2E bus tests` step.**

## E2E test philosophy

**Zero tolerance for silent failures.** Every test must hard-fail on any unexpected condition. Specifically:

- **NO graceful fallbacks** — never accept a degraded result (e.g. "API unavailable, moving on")
- **NO conditional PASS** — either all assertions hold or the test exits non-zero
- **NO `⚠️` warnings in place of asserts** — if a condition matters, assert it
- **Assert every phase** — if a test exercises a multi-step flow (publish → process → result), every step must be verified with a hard assertion
- **Infrastructure failures are test failures** — if an LLM model is missing or a service is down, the test fails. Fix the infra, don't paper over it

Every E2E test must print exactly `=== ALL ... TESTS PASSED ===` on stdout or crash. No partial passes.

## Editing reliability

Edits can silently fail to persist to the working tree (observed in practice:
subagents reported "fixed + tests passing" while their file changes were
absent from `git status`). A tool's success message is not proof the change
landed. Before reporting a task done — and especially before committing:

- **Verify the write landed.** After editing, re-read the changed lines or run
  `git diff --stat <file>` / `git status` and confirm the expected change is
  present in the working tree.
- **Trust tests, not claims.** For a fix, the confirming test must be observed
  to FAIL before the edit and PASS after it. If it passes without your change,
  your edit almost certainly didn't land — re-apply and re-verify.
- **Confirming tests must be real.** A test that passes trivially (or wasn't
  actually written) does not prove the fix exists. Check that the test file/lines
  exist on disk.
- Never report "done" or commit based solely on an agent's success message.


- Use Mermaid for sequence diagrams in markdown.
- Always add ADRs when appropriate.
- Cross-link related docs and ADRs as appropriate.
