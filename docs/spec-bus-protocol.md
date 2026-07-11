# Bus Protocol Specification

`cafe-bus` is the central message broker. All services connect to it as clients over
a Unix domain socket (local) or [iroh QUIC](#iroh-transport) (remote P2P).

**Note on wire format**: The bus uses a pluggable `BusCodec` trait
([`cafe-types/src/codec.rs`](../cafe-types/src/codec.rs)).
The default codec is newline-delimited JSON (NDJSON), described below.
With the `bincode-codec` feature, the bus uses length-prefixed bincode v2
(4-byte little-endian length prefix + bincode payload).
All services must agree on the same codec — the bus server is instantiated
with one at compile time.

---

## Connection

**Unix socket path:** `$CAFE_BUS_SOCKET` (default `/tmp/cafe-bus.sock`)

Each client opens a persistent TCP-like connection to the socket. There is no
authentication at the socket level — restrict socket permissions via filesystem
(mode 0600, owned by the service user).

**iroh (remote):** See [iroh transport](#iroh-transport) below.

---

## iroh transport

iroh provides P2P QUIC connectivity so clients can connect to a single cafe-bus
from remote machines, through NATs and firewalls. See [ADR-118](adr-118-iroh-transport.md).

### Bus setup

Set `CAFE_BUS_IROH_SECRET_KEY` to a hex-encoded (64 hex chars) Ed25519 secret key.
The bus will bind an iroh endpoint alongside its Unix socket:

```bash
export CAFE_BUS_IROH_SECRET_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
cafe-bus
```

The bus writes its `EndpointAddr` (public key, relay URL, IP addresses) to
`<socket-path>.iroh-addr` after startup. Clients can use this file for discovery.

### Client setup (cafe-cli)

Three ways to connect with cafe-cli:

1. **Auto-discovery via addr file** — uses the bus-written `.iroh-addr` file:
   ```
   cafe-cli --bus /path/to/cafe-bus.sock create-session --agent default
   ```

2. **Explicit key + relay** — specify the bus endpoint ID and a relay URL:
   ```
   cafe-cli --bus-iroh-key <bus-public-key-hex> --bus-iroh-relay https://euc1-1.relay.n0.iroh.link./ create-session --agent default
   ```

3. **Env vars** — `CAFE_BUS_IROH_KEY`, `CAFE_BUS_IROH_RELAY`, `CAFE_BUS_IROH_ALPN`:
   ```
   CAFE_BUS_IROH_KEY=<key> CAFE_BUS_IROH_RELAY=<url> cafe-cli create-session --agent default
   ```

### Client setup (SDK)

```rust
use cafe_sdk::bus::{BusClient, IrohConfig};
use std::str::FromStr;

// From CLI args or env
let cfg = IrohConfig::from_cli(
    Some("bus-public-key-hex"),      // --bus-iroh-key
    Some("https://relay.url"),       // --bus-iroh-relay
    None,                            // --bus-iroh-alpn (default: cafe-bus/0)
).expect("valid key");

// Or from the bus's addr file
let json = std::fs::read_to_string("/tmp/cafe-bus.sock.iroh-addr")?;
let cfg = IrohConfig::from_bus_addr_json(&json).expect("valid addr");

let client = BusClient::from_iroh_config(cfg).await?;
// use client.publish(), client.subscribe(), etc. — same API as Unix
```

### Adding iroh to a service

1. Enable the feature on cafe-sdk:
   ```toml
   cafe-sdk = { path = "../cafe-sdk", features = ["bus-client", "iroh-client"] }
   ```

2. Construct the client at startup:
   ```rust
   let client = if let Some(cfg) = IrohConfig::from_cli(
       cli_bus_iroh_key.as_deref(),
       cli_bus_iroh_relay.as_deref(),
       cli_bus_iroh_alpn.as_deref(),
   ) {
       BusClient::from_iroh_config(cfg).await?
   } else {
       BusClient::unix(&socket_path)
   };
   ```

### Architecture

```
Remote machine                  Local machine
┌──────────────┐               ┌──────────────────────────────────┐
│ cafe-llm     │               │ cafe-bus                         │
│ (IrohTransport)│←─ QUIC ───→│ /tmp/cafe-bus.sock ←─ cafe-store │
│              │    P2P       │ + iroh endpoint   ←─ cafe-server │
└──────────────┘    via n0    │                   ←─ cafe-tui    │
                    relay     └──────────────────────────────────┘
```

Both transports feed the same protocol (identical `ClientMessage`/`ServerMessage`
NDJSON) — the bus treats all connections identically regardless of transport.

### Relay servers

By default, the bus and clients use [n0's public relay network](https://n0.computer).
No infrastructure needed — both sides auto-discover the nearest relay. For production,
you can run your own relay server and configure it via `--bus-iroh-relay`.

### Peer-to-peer flow

1. Both bus and client bind an iroh endpoint (connects to n0 relay)
2. Client calls `endpoint.connect(bus_addr)` — routed through relay
3. QUIC handshake completes (hole-punched direct if possible, relayed if not)
4. Client opens a bidirectional stream, writes a `SetMeta` message
5. Bus accepts the stream, sends `Connected`, starts processing messages
6. Same NDJSON protocol runs over the QUIC stream

### Limitations

- No peer whitelist yet — any endpoint with the bus's public key + relay URL can connect
- All clients share one bus endpoint; the bus does not federate between multiple instances
- Relay servers are an external dependency (n0 public relays by default)

---

## Message format

Every message is a single JSON object followed by `\n`. No multi-line JSON.
Maximum message size: 16 MB.

### Client → Bus messages

#### subscribe

Subscribe to all chunks published to a session's output stream.

```json
{ "op": "subscribe", "session_id": "abc123" }
```

After subscribing, the bus will replay all historical chunks for the session
(oldest first), then stream new chunks as they arrive.

#### subscribe_all

Subscribe to all chunks on all sessions. Used by `cafe-store`.

```json
{ "op": "subscribe_all" }
```

#### publish

Publish a chunk to a session's input stream.

```json
{
  "op": "publish",
  "session_id": "abc123",
  "chunk": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "content_type": "text",
    "content": "Hello",
    "data": null,
    "mime_type": null,
    "producer": "com.nominal.cafe-server",
    "annotations": { "chat.role": "user" },
    "timestamp": 1717123456789
  }
}
```

#### create_session

Create a new session. Returns a `session_created` event.

```json
{
  "op": "create_session",
  "session_id": "abc123",
  "agent_id": "default",
  "config": {
    "backend": "ollama",
    "model": "gemma3:1b",
    "system_prompt": "You are a helpful assistant."
  }
}
```

#### delete_session

```json
{ "op": "delete_session", "session_id": "abc123" }
```

#### list_sessions

```json
{ "op": "list_sessions" }
```

#### subscribe_filtered

Subscribe to chunks matching a filter on a session. Used by `cafe-binary-store`.

```json
{
  "op": "subscribe_filtered",
  "session_id": "abc123",
  "content_types": ["BinaryRef"]
}
```

Supported filter fields: `content_types` (array of content type strings).

#### ping

Keep-alive. Bus responds with `pong`.

```json
{ "op": "ping" }
```

---

### Bus → Client messages

#### connected

Sent immediately after connecting, carrying the client's assigned connection ID.

```json
{ "event": "connected", "connection_id": "conn-42" }
```

The connection ID is used for `direct_to` routing (see `cafe.direct_to` annotation).

#### chunk

A chunk being delivered to a subscriber.

```json
{
  "event": "chunk",
  "session_id": "abc123",
  "chunk": { ...chunk fields... }
}
```

#### session_created

```json
{
  "event": "session_created",
  "session_id": "abc123",
  "agent_id": "default"
}
```

#### session_deleted

```json
{ "event": "session_deleted", "session_id": "abc123" }
```

#### sessions_list

```json
{
  "event": "sessions_list",
  "sessions": [
    { "session_id": "abc123", "agent_id": "default", "message_count": 14 }
  ]
}
```

#### error

```json
{
  "event": "error",
  "session_id": "abc123",
  "message": "Agent not found: foobar",
  "code": "AGENT_NOT_FOUND"
}
```

#### history_complete

Sent to a subscriber after all historical chunks have been replayed and live
streaming begins.

```json
{ "event": "history_complete", "session_id": "abc123", "count": 42 }
```

#### pong

```json
{ "event": "pong" }
```

---

## Replay behaviour

When a client subscribes to a session, the bus immediately sends all historical
output chunks for that session (sourced from `cafe-store`), then continues streaming
live chunks. Each historical chunk is sent as a normal `chunk` event.

The client can detect the transition from history to live by watching for a special
sentinel:

```json
{ "event": "history_complete", "session_id": "abc123", "count": 42 }
```

---

## Error codes

| Code                  | Meaning                                       |
|-----------------------|-----------------------------------------------|
| `AGENT_NOT_FOUND`     | Requested agent_id is not registered          |
| `SESSION_NOT_FOUND`   | session_id does not exist                     |
| `SESSION_EXISTS`      | create_session called with an existing ID     |
| `INVALID_MESSAGE`     | Malformed JSON or missing required fields     |
| `PAYLOAD_TOO_LARGE`   | Message exceeds 16 MB                         |

---

## Implementation notes for cafe-bus

- Use `tokio::net::UnixListener` for the socket.
- Each connected client gets a `tokio::task` and a `tokio::sync::broadcast` receiver.
- Sessions are stored in a `HashMap<String, SessionState>` behind an `Arc<RwLock<...>>`.
- `SessionState` contains: `agent_id`, `Vec<Chunk>` (history), `broadcast::Sender<Chunk>`.
- Publishing to a session sends to the broadcast channel; all subscribers receive it.
- History is the authoritative state: on subscribe, replay `history` then hand off the
  live receiver.
- The bus does NOT call LLM backends. It just routes chunks.
