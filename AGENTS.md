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

## Doc rules

- Use Mermaid for sequence diagrams in markdown.
- Always add ADRs when appropriate.
- Cross-link related docs and ADRs as appropriate.
