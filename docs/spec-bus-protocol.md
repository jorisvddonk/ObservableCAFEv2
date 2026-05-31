# Bus Protocol Specification

`cafe-bus` is the central message broker. All services connect to it as clients over
a Unix domain socket. The protocol is newline-delimited JSON (NDJSON).

---

## Connection

Socket path: `$CAFE_BUS_SOCKET` (default `/tmp/cafe-bus.sock`)

Each client opens a persistent TCP-like connection to the socket. There is no
authentication at the socket level — restrict socket permissions via filesystem
(mode 0600, owned by the service user).

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

#### ping

Keep-alive. Bus responds with `pong`.

```json
{ "op": "ping" }
```

---

### Bus → Client messages

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
