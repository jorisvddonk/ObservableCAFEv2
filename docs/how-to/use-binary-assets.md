# How to: Work with binary assets

This guide shows you how to move bytes — audio, images, files — through the bus.
Binary data is never put in a chunk's `content`; instead, chunks carry a
`binary-ref` that points at bytes stored in `cafe-binary-store`.

---

## The mental model

- **`binary` chunk** — raw bytes inline, base64-encoded on the wire. Fine for
  small payloads.
- **`binary-ref` chunk** — an announcement by reference: the actual bytes live in
  `cafe-binary-store`, reachable via an HTTP URL. This is what TTS, STT, and
  image services use.
- **Mutation chunks** — a producer publishes a `binary-ref`, and the binary store
  replies with a *mutation* that injects the `write_url` / `read_url` / tokens as
  late-bound annotations on the original chunk.

Full field and key tables: [Data model spec, §1 and Binary assets](../spec-cafe.md).

## 1. Upload bytes and get a read URL (the low-level flow)

1. **Publish a `binary-ref` chunk** (transient, holding the connection open to
   catch the mutation):

   ```sh
   cafe-cli publish "$SESSION" --binary-ref --mime audio/wav --transient --wait 10
   ```

2. **Read the mutation** — the binary store broadcasts a mutation chunk carrying
   `binary.write_url`, `binary.write_token`, `binary.read_url`,
   `binary.read_token` for the referenced chunk.

3. **Upload the bytes** to `binary.write_url` with the write token:

   ```sh
   curl -X POST "$WRITE_URL" -H "Authorization: Bearer $WRITE_TOKEN" \
        --data-binary @audio.wav
   ```

4. **Download** from `binary.read_url` with the read token (no expiry on reads).

## 2. The convenient CLI path

```sh
cafe-cli publish "$SESSION" --file photo.png --mime image/png
```

This uploads the file to the binary store and publishes the `binary-ref`. To read
it back, use the store subcommand:

```sh
cafe-cli store --help
```

## 3. From a Rust service

The SDK wraps the whole dance. Publishing a binary ref and streaming bytes uses
`cafe-sdk`'s bus + HTTP helpers; the binary-store annotations are merged
client-side from mutation chunks. See the `cafe-tts` service (`worker.rs`,
`executor.rs`) for a worked example — it publishes `BinaryRef` chunks with a
`binary.byte_size` annotation.

## 4. From the web

The browser never talks to the bus. `cafe-server` proxies binary content:

- `GET /api/sessions/:id/chunks/:chunk_id/binary` — returns a JSON
  `{ "url": "<binary-store read URL>" }` (the read token is already embedded),
  or a redirect for direct use in `<audio>` / `<img>` tags.
- The `cafe-web-sdk` handles `binary-ref` chunks and merging mutations
  (`getBinaryUrl()`, `BinaryRefContent`).

## Range requests and large files

`cafe-binary-store` is streaming and HTTP-native (range requests, JWT auth,
garbage collection). Clients that need partial reads use standard `Range`
headers against the read URL. See [ADR-107](../adr-107-binary-streaming.md).

## Trust and security

- Write tokens are short-lived; read tokens don't expire.
- Binary URLs are only handed out by mutation after the producer published a
  `binary-ref` — nothing is readable without having been announced.
- Untrusted fetched content (e.g. web fetches) is marked
  `security.requires-review` and must be explicitly trusted before an LLM sees it.
  See [How to: the web-fetch flow](../spec-http-api.md#fetch-web-content-untrusted).

---

## Reference

- [ADR-107: Binary streaming](../adr-107-binary-streaming.md)
- [ADR-114: Binary upload completion event](../adr-114-binary-upload-completion-event.md)
- [ADR-115: HTTP binary-ref rejection](../adr-115-http-binary-ref-rejection.md)
- [`cafe.binary.*` annotation keys](../cafe-annotations.md#binary-store)
- [How to: run the stack](run-the-stack.md) — `cafe-binary-store` runs on port 4002
