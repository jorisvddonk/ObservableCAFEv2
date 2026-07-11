# ADR-118: iroh transport for remote bus connectivity

**Status**: Implemented (commit TBD)

**Context**: The cafe-bus currently listens exclusively on a Unix domain socket (`/tmp/cafe-bus.sock`). This confines all services to a single host (ADR-001). There is no way to run services on remote machines — a GPU-bound `cafe-comfy` or `cafe-llm` must be colocated with the bus. Similarly, `cafe-tui` and `cafe-cli` cannot connect from outside the LAN without a VPN or reverse proxy.

[iroh](https://crates.io/crates/iroh) (v1.0.2, n0-computer) is a P2P QUIC library that dials connections by public key instead of IP address. It provides NAT traversal (hole-punching), relay server fallback, and encrypted bidirectional streams. It is MIT/Apache-2.0 licensed, v1.0+ stable, and compatible with the project's `tokio 1.x` dependency.

**Decision**: Add iroh as a second transport for cafe-bus, transparent to the message protocol. The bus continues to accept Unix socket connections (local services). Additionally, it binds an `iroh::Endpoint` and accepts QUIC connections from remote peers. Both transports feed identical messages into the same bus internals.

In cafe-sdk, abstract the transport behind a `BusTransport` trait so that `BusClient` can connect via either Unix socket or iroh without duplicating protocol logic. Defer bincode serialization to a future ADR.

### Design

#### Trait abstraction

```rust
/// Abstraction over the underlying connection to the bus.
/// Implemented by Unix socket (existing) and iroh QUIC (new).
trait BusTransport {
    type Reader: AsyncRead + Unpin + Send + 'static;
    type Writer: AsyncWrite + Unpin + Send + 'static;

    async fn connect(&self) -> Result<(Self::Writer, Self::Reader)>;
    fn description(&self) -> &str; // for logging/diagnostics
}
```

- `UnixSocketTransport` wraps a socket path and calls `UnixStream::connect()`.
- `IrohTransport` wraps an `iroh::Endpoint` + bus `EndpointAddr` and calls `ep.connect()`.
- `BusClient` is parameterized on `T: BusTransport`. Existing convenience methods (`publish`, `subscribe_session`, etc.) work against `T` generically — they call `self.transport.connect()`, split into read/write halves, and run the exact same protocol loop.
- `SessionSubscription` stores `T::Writer` instead of the current concrete `OwnedWriteHalf`.

#### Bus iroh listener

- Configured via env vars: `CAFE_BUS_IROH_SECRET_KEY` (hex-encoded Ed25519 seed), `CAFE_BUS_IROH_ALLOWED_PEERS` (comma-separated `EndpointId`s), `CAFE_BUS_IROH_ALPN` (default `"cafe-bus/0"`).
- On startup, if `CAFE_BUS_IROH_SECRET_KEY` is set, the bus binds an `iroh::Endpoint` with the specified ALPNs.
- A background task calls `ep.accept()` in a loop. Each accepted connection is fed into the existing `handle_client` function — zero changes to session registry, chunk routing, `direct_to`, or ephemeral session logic.
- Connection IDs are namespaced: `iroh:<peer_prefix>:c-N` for iroh connections, `unix:c-N` for Unix socket connections. The bus rejects iroh connections from peers not in the allowed list.
- If `CAFE_BUS_IROH_SECRET_KEY` is not set, iroh is disabled and the bus behaves identically to today.

#### SDK client

- `BusClient<T: BusTransport>` replaces the current `BusClient { socket_path: Arc<String> }`.
- A convenience constructor `BusClient::unix(socket_path)` produces `BusClient<UnixSocketTransport>`.
- A convenience constructor `BusClient::iroh(bus_id, relay_url, alpn)` produces `BusClient<IrohTransport>`.
- `run_with_reconnect()` already accepts a generic closure — no change needed.
- `wait_for_bus()` remains Unix-socket-specific (polling `UnixStream::connect`). For iroh clients, bus readiness is verified by a successful `connect()` on first use, since iroh nodes may not be reachable until both sides are online.

#### Service binaries

Each service that needs remote connectivity gains optional CLI flags:

```
--bus-iroh-key <EndpointId>    # Bus public key to dial
--bus-iroh-relay <RelayUrl>    # Relay URL (default: n0 defaults)
--bus-iroh-alpn <ALPN>         # ALPN (default: cafe-bus/0)
```

When provided, the service constructs `BusClient<IrohTransport>`. Otherwise, it falls back to `BusClient<UnixSocketTransport>` with the socket path from `CAFE_BUS_SOCKET`. No service is forced to use iroh.

#### cafe-server remote access

cafe-server optionally accepts iroh connections for remote TUI/CLI access, bridging them to bus messages via the existing handler pattern (analogous to the WebSocket bridge, ADR-113). This keeps auth, session management, and SSE streaming intact for remote clients.

### Wire format

Same NDJSON framing as the Unix socket transport. The existing `BusCodec` trait (already generic over framed streams) works unchanged over QUIC bidirectional streams. No format negotiation needed at this stage.

### Peer identity and access control

- Each remote service generates an Ed25519 `SecretKey` on first run, persisted to disk.
- The bus operator adds the service's `EndpointId` (derived public key) to `CAFE_BUS_IROH_ALLOWED_PEERS`.
- The bus rejects connections from unlisted peers at accept time.
- This replaces Unix socket permissions (mode `0o600`) as the access control mechanism for remote connections.

### Consequences

**Positive:**
- Services can run on any machine (GPU box, cloud VM) and connect to a single bus.
- Remote TUI/CLI access without VPN, reverse proxy, or port forwarding.
- QUIC gives encrypted, multiplexed streams with NAT traversal built in.
- Transport abstraction makes future transports (WebSocket, TCP) trivial to add.
- Existing local-only deployment path is unchanged — iroh is opt-in.

**Negative:**
- ~80 transitive crate dependencies added to the workspace (QUIC stack, TLS, crypto).
- Increased compile time and binary size (~3-5 MB per binary that links iroh).
- Peer key management is manual — services must generate keys, operator must whitelist.
- iroh relay servers introduce an external runtime dependency (can be mitigated by running a self-hosted relay if needed).
- The `BusTransport` trait refactoring touches most service binaries (constructor change from `BusClient::new(path)` to `BusClient::unix(path)`), though the changes are mechanical.

### Alternatives considered

- **iroh-blobs / iroh-gossip**: Higher-level iroh protocols for content-addressed blob transfer and pub-sub overlay networks. Rejected — the existing bus protocol suffices for message routing; iroh is used only for the transport layer.
- **Separate `IrohBusClient` struct instead of a trait**: Simpler to implement (no refactoring of existing code), but duplicates the entire protocol layer (~500 lines of publish/subscribe/session logic). The trait approach is more work upfront but eliminates duplication and makes future transports zero-cost.
- **gRPC or NATS instead of iroh**: Rejected in ADR-001. Introduces operational complexity (message broker dependency). iroh is a library, not a service — aligns with the zero-infrastructure philosophy.
- **WebRTC for remote access**: More complex than iroh, requires STUN/TURN signaling infrastructure, and doesn't provide a unified SDK abstraction. iroh handles signaling, hole-punching, and relay transparently.
- **Run iroh relay self-hosted only**: Rejected for initial implementation. N0's public relays reduce operational burden. Self-hosted relay can be added later if needed.

### Implementation plan

1. Add `BusTransport` trait to `cafe-sdk`, implement `UnixSocketTransport`, refactor `BusClient` to be generic over it. Update all service binary constructors (`BusClient::new(path)` → `BusClient::unix(path)`).
2. Add `iroh` dependency to cafe-bus, implement iroh listener behind env-var gate.
3. Add `IrohTransport` to cafe-sdk, implement `iroh-client` feature flag.
4. Add `--bus-iroh-*` CLI flags to service binaries, construct `BusClient<IrohTransport>` when flags are present.
5. Add iroh listener to cafe-server for remote TUI/CLI access.
6. (Future ADR): Enable bincode-codec for compact wire format over iroh streams.
