# Cafe-Web Gotchas

## Chunk mutations are invisible to `messages`

**What**: Binary-store publishes `read_url`/`read_token` on **separate mutation chunks** (`content_type: null`, no `chat.role`) with `cafe.mutates.target_id` pointing at the `BinaryRef` chunk. These mutation chunks get filtered out of the `messages` array because the chat filter only lets through `text`/`binary`/`binary-ref` with a `chat.role`.

**Why it breaks**: On page refresh, `switchSession` loads history, filters for chat chunks, then calls `setMessages(filtered)`. The mutations are not in the filtered array, so `read_url` never reaches the `BinaryRef`. Audio is invisible.

**Fix**: Apply mutations on the **full** history array (`applyMutations(fullChunks)`) before filtering. The hook (`useSessions.ts`) owns this, not the store.

## Zustand mutation handlers must produce new references

**What**: The store's `appendChunk` and `finaliseStream` have mutation-merge logic (for live SSE chunks). Originally they mutated `target.annotations` in-place and returned `s` — the same state object reference.

**Why it breaks**: Zustand's `set()` with `return s` skips the re-render because the reference didn't change. The chunk looks updated in memory but React never shows the new annotations.

**Fix**: Use `.map()` to produce a new array and `{ ...c, annotations: { ...c.annotations } }` to produce new chunk objects. Return the new state object `{ messages: newArr, allChunks: newArr }`.

Both `messages` AND `allChunks` must be updated — not just `allChunks`.

## Mutation chunks must be in both `messages` and `allChunks` for `applyMutations`

**What**: The store's `setMessages` and `setAllChunks` both call `applyMutations(chunks)`. But `applyMutations` scans its **own input** for mutation chunks — it can't see chunks in the other array.

**Why it breaks**: If `setMessages(filteredChatChunks)` is called and mutation chunks were filtered out, `applyMutations` finds nothing to merge.

**Fix**: The hook applies mutations on the full chunk list before splitting. The store's `applyMutations` in `setMessages`/`setAllChunks` is now redundant but kept as a safety net for cases where the full list is passed.

## `publish_direct` chunks are connection-private, not broadcast

**What**: Cafe-binary-store sends write credentials via `publish_direct` to the producer's connection. These chunks bypass session subscribers and never appear in session history.

**Why it matters**: E2E tests using raw socket subscriptions can't see `publish_direct` chunks. The upload lifecycle verification in tests must only assert phases visible via broadcast (read credentials + completion), not write credentials.

## `BusClient::publish` opens a temp connection

**What**: `BusClient::publish()` opens a fresh connection, sends, shuts down the writer, and drops the reader immediately. The bus sets `cafe.source.connection` to this temp connection's ID.

**Why it breaks**: Any service trying to `publish_direct` a reply (e.g. binary-store sending write credentials back) gets `TARGET_NOT_FOUND` because the source connection is dead.

**Fix**: Use `subscribe_session()` + `sub.publish()` — the subscription's persistent connection stays alive for replies. `BusClient::publish()` is deprecated.
