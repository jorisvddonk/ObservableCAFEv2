# ADR-119: Binary codec with protocol negotiation

**Status**: Implemented (commit TBD)

**Date**: 2026-07-11

**Context**: The bus currently uses NDJSON for all wire messages. A `BincodeLengthPrefixCodec` exists in `cafe-types` (behind `bincode-codec` feature) but is never used — the bus hardcodes `JsonLineCodec` and no codec negotiation exists. With iroh adding remote connectivity (ADR-118), clients want a binary codec for higher throughput and smaller wire size. The challenge: codec negotiation messages themselves must be sent over the connection, but you need a codec before you can read them — the chicken-and-egg problem.

**Decision**: Every connection starts with JSON. The `SetMeta` handshake message is extended with an optional `codecs` field listing client codec preferences. If the bus sees `codecs`, it picks the first codec it supports, sends `CodecSet`, and both sides switch. If no `codecs` field, the bus treats it as a legacy client and continues with JSON.

Old buses ignore the unknown `codecs` field (serde default) and send `Connected` as before — new clients detect this and fall back to JSON. Old clients don't send `codecs`, so new buses handle them identically.

### Design

#### Wire format

Client → Bus (always JSON):
```json
{"op":"set_meta","role":null,"codecs":["bincode","json"]}
```

Bus → Client (negotiated, always JSON):
```json
{"event":"codec_set","codec":"bincode","connection_id":"c-3"}
```

Bus → Client (legacy fallback, always JSON):
```json
{"event":"connected","connection_id":"c-3"}
```

After `codec_set`, both sides switch to the negotiated codec for all subsequent messages on that connection. Every new connection re-negotiates (fresh handshake each time — the bus has no client identity, and state is per-connection).

#### Message types (cafe-types)

```rust
// Extension to ClientMessage::SetMeta
SetMeta {
    role: Option<String>,
    #[serde(default)]
    codecs: Option<Vec<String>>,
}

// New ServerMessage variant
CodecSet {
    codec: String,
    connection_id: String,
}
```

#### Bus-side: handle_connection

New function in `cafe-bus/src/client.rs` replaces the hardcoded `handle_client::<JsonLineCodec>` in `main.rs`. The function:

1. Buffers data from the raw reader, feeds it to `JsonLineCodec::decode`
2. Decodes the first client message as JSON
3. Checks for `SetMeta.codecs`:
   - `Some(cs)` → picks first matching codec from `["json"`, and if `bincode-listener` is enabled, `"bincode"]`, sends `CodecSet`, dispatches to `client_loop::<BincodeLengthPrefixCodec>` or `JsonLineCodec`
   - `None` → legacy, sends `Connected`, dispatches to `client_loop::<JsonLineCodec>`
4. Same cleanup logic as the original `handle_client`

The `client_loop` function now takes a `Frames<C, R>` directly (instead of `R`), allowing the caller to inject a pre-built reader with buffered data.

#### SDK-side: ClientCodec + negotiate

```rust
pub enum ClientCodec {
    Json,
    #[cfg(feature = "bincode-client")]
    Bincode,
}
```

`BusClient` stores two new fields:
- `preferred_codecs: Vec<ClientCodec>` — ordered codec preferences (default: `[Json]`)
- `negotiated_codec: Arc<Mutex<Option<ClientCodec>>>` — lazily set on first use

`connect_with_role<C>` (the low-level connection opener) now **always** sends `SetMeta` as JSON (regardless of `C`) with the `codecs` field populated from preferences. It reads the response as JSON, verifies the negotiated codec matches `C::NAME`, then extracts the raw reader via `BusReader::into_inner()` and creates a `BusReader<C>` for subsequent messages.

`Negotiate()` opens a dedicated negotiation connection on first use, determines the codec, caches the result. All convenience methods (`publish`, `subscribe`, etc.) call `negotiate()` then dispatch to the appropriate `_with_codec::<C>` variant.

`SessionSubscription` no longer carries the `C: BusCodec` type parameter — it stores a `ClientCodec` value and dispatches in `publish()`. This keeps the public API stable (all existing callers use default type parameters).

#### Connection flow

```
Client                         New Bus                      Old Bus
  │                              │                            │
  │ SetMeta (codecs: [...])     │                            │
  │ ─────────────────────────>  │                            │
  │                              │ (detects codecs)          │
  │ CodecSet { codec }          │                            │
  │ <─────────────────────────  │                            │
  │ [switch to negotiated]      │ [switch to negotiated]     │
  │ <══ codec messages ══════> │                            │
  │                              │                            │
  │          OR against old bus:                              │
  │ SetMeta { codecs: [...] }   │                            │
  │ ────────────────────────────────────────────────────────>│
  │ Connected { ... }           │ (ignores codecs field)    │
  │ <────────────────────────────────────────────────────────│
  │ [no CodecSet → Json]        │ [stays on Json]           │
```

### Feature flags

| Crate | Feature | Default | What it gates |
|-------|---------|---------|---------------|
| `cafe-types` | `bincode-codec` (existing) | off | `BincodeLengthPrefixCodec`, `raw-binary-data` on `Chunk.data` |
| `cafe-bus` | `bincode-listener` (new) | off | Bus can accept `"bincode"` as a negotiated codec |
| `cafe-sdk` | `bincode-client` (new) | off | `ClientCodec::Bincode` variant, ability to request bincode |

### Files changed

| File | Change |
|------|--------|
| `cafe-types/src/envelope.rs` | Extend `SetMeta` with `codecs`, add `CodecSet` to `ServerMessage` |
| `cafe-bus/src/client.rs` | Add `handle_connection()` with JSON-first negotiation; make `client_loop` take `Frames`; add `Frames::with_buf/into_parts` |
| `cafe-bus/src/main.rs` | Replace `handle_client::<JsonLineCodec>` with `handle_connection` in both Unix and iroh paths |
| `cafe-bus/Cargo.toml` | Add `bincode-listener` feature |
| `cafe-sdk/src/bus/mod.rs` | Add `ClientCodec` enum; add `preferred_codecs`/`negotiated_codec` to `BusClient`; add `negotiate()`; modify `connect_with_role` for JSON-first handshake; de-genericize `SessionSubscription`; dispatch all convenience methods |
| `cafe-sdk/Cargo.toml` | Add `bincode-client` feature |
| `Cargo.toml` | Fix misplaced `members` under `[profile.test]` |
| `tests/ephemeral-sessions-e2e.py` | Update `BusConnection` to send `SetMeta` before reading (new bus reads-first semantics) |

### Consequences

**Positive:**
- 4-byte LE length + bincode replaces JSON text + newline — smaller and faster
- `raw-binary-data` avoids base64 overhead for binary chunks
- Fully backward compatible — old buses ignore `codecs`, old clients don't send it
- No magic bytes, no race conditions, no separate ports/ALPN
- Compile-time dispatch (no dyn overhead) via two monomorphized codec paths

**Negative:**
- First message always JSON even for bincode connections (negligible: one small message)
- Two codec paths to test and maintain in the bus + SDK
- Convenience methods match on `ClientCodec` enum rather than being purely generic
- Bus holds off sending `Connected` until first client message is received (changed timing; see ephemeral test fix)

### Alternatives rejected

- **Magic-byte prefix per frame** (e.g., `0x00` for bincode, `{` for JSON): per-frame overhead, doesn't solve initial handshake
- **Separate ports/ALPN**: fragments transport, can't mix codecs on same transport
- **Trait-object codec (`Box<dyn BusCodec>`)**: `BusCodec` methods are generic (not object-safe), would require major refactor
- **Bus sends Connected first, then reads + negotiates**: the client would already be past the handshake and not expect `CodecSet`. The read-first approach is cleaner (client writes SetMeta first, then reads the response).

**Supersedes**: ADR-118's note "Defer bincode serialization to a future ADR"
