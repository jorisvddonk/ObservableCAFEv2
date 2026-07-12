# ADR-120: iroh peer-ID allowlist for the bus

**Status**: Implemented

**Date**: 2026-07-12

**Context**: ADR-118 added iroh as a second bus transport and stated "The bus rejects
iroh connections from peers not in the allowed list" — but that enforcement was
never implemented. As shipped, `cafe-bus/src/main.rs::run_iroh_listener` calls
`ep.accept()` and hands every incoming connection straight to `handle_connection`,
so **anyone who learns the bus `EndpointId` (written to the `.iroh-addr` file) gets
full bus access**. The `EndpointId` is a public key / routing address by design
(see iroh docs), not a secret, so relying on its secrecy is security-through-obscurity.

Two problems had to be solved:
1. **Clients used ephemeral iroh keys** (`IrohConfig` never set a client secret key),
   so each connection presented a *different* `remote_id()` — a peer-ID list is
   useless against random IDs.
2. **No enforcement point** existed; the listener accepted unconditionally.

**Decision**: Enforce a **database-backed allowlist of peer IDs** at the iroh
handshake, using iroh's `EndpointHooks` (`after_handshake`). The bus rejects any
incoming connection whose `remote_id()` is not in the list, with QUIC close code 403.

- **Clients get stable identities**: `CAFE_BUS_IROH_CLIENT_SECRET_KEY` (hex Ed25519
  seed) is read by `IrohConfig::from_cli` / `from_bus_addr_json` in `cafe-sdk` and
  applied via `with_secret_key`. With it set, a client's `remote_id()` is fixed and
  registerable. Without it, the client stays ephemeral (previous behavior).
- **Allowlist storage**: a dedicated SQLite DB (`iroh_allowlist` table:
  `peer_id TEXT PRIMARY KEY, label TEXT, created_at INTEGER`), owned by `cafe-bus`
  (new `cafe-bus/src/allowlist.rs`). Path via `CAFE_BUS_IROH_ALLOWLIST_DB`.
- **Hot reload**: an in-memory `HashSet` cache is refreshed from the DB on a 5s
  interval (`spawn_refresh_task`), so CLI edits apply without restarting the bus.
  The hook does an O(1) cache lookup — no per-connection DB hit.
- **Fail-closed**: when the allowlist is enabled but the table is empty, *all*
  iroh connections are rejected (logs a warning). `CAFE_BUS_IROH_ALLOWLIST_DISABLED=1`
  bypasses the check for emergencies / local dev. If `CAFE_BUS_IROH_ALLOWLIST_DB`
  is unset, the listener behaves exactly as before (no gate).
- **Scope**: iroh only. The Unix socket keeps its `0o600` file permissions.
- **Admin CLI**: `cafe-cli iroh-allowlist {add,remove,list,my-id}` operates on the
  same DB. `my-id` prints a client's stable peer ID (generating + printing a secret
  if `CAFE_BUS_IROH_CLIENT_SECRET_KEY` is unset) so operators can register it.

This refines ADR-118's "access control" section, which proposed a comma-separated
`CAFE_BUS_IROH_ALLOWED_PEERS` env var. Env-var was rejected in favor of a DB + CLI
because it allows dynamic updates without a bus restart and is easier to provision
from automation.

**Consequences**:
- Remote iroh connections are now genuinely access-controlled, not just
  obscurity-protected.
- Each remote service must be provisioned with a stable secret key and have its
  peer ID registered in the allowlist DB before it can connect.
- New operational surface: the allowlist DB must be provisioned and backed up
  alongside the bus.
- `cafe-bus` gains a `sqlx` (sqlite) dependency, gated behind the `iroh-listener`
  feature.

**Alternatives considered**:
- **Static peer-ID env var (`CAFE_BUS_IROH_ALLOWED_PEERS`)** as in ADR-118: simpler,
  but requires a bus restart to change, and is awkward to manage across many peers.
- **Shared pre-shared token at the protocol level** (first bus message must carry a
  token): works with ephemeral clients and is arguably simpler, but the user
  specifically wanted a peer-ID allowlist, and a peer-ID list is stronger (per-client
  revocation, no shared secret to leak).
- **iroh's documented `auth-hook` pattern** (separate auth ALPN + pre-auth): most
  thorough, but heavier to implement and overkill for a single trusted bus. The
  `EndpointHooks::after_handshake` reject-by-`remote_id` achieves the needed control
  with far less machinery.

**Implementation**: `cafe-bus/src/allowlist.rs`, wired into `run_iroh_listener`
(`cafe-bus/src/main.rs`); client key support in `cafe-sdk/src/bus/iroh_transport.rs`;
admin commands in `cafe-cli/src/main.rs`. Unit tests cover the cache and hook
decision logic (87 cafe-bus tests passing).
