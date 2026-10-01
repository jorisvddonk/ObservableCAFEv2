import type { Chunk } from 'cafe-web-sdk';
import {
  CHAT_IS_STREAMING,
  CHAT_MODEL,
  CHAT_STREAM_COMPLETE,
  FLOW_SIGNAL,
  FLOW_TARGET_CHUNK_ID,
  FLOW_TOMBSTONE,
} from 'cafe-web-sdk';

/**
 * A per-token delta chunk carries `chat.is_streaming` but NOT `chat.model`.
 *
 * The LLM evaluator (`cafe-llm/src/evaluator.rs`) emits TWO kinds of
 * `is_streaming` chunks:
 *   1. per-token delta chunks (transient, no model annotation)
 *   2. a single full `response_chunk` carrying the entire text, which also has
 *      `chat.is_streaming` set AND a `chat.model` annotation.
 *
 * The final `stream_complete` chunk carries `chat.stream_complete` and has no
 * useful `content`.
 *
 * Treating both #1 and #2 as streaming tokens would double-count the text.
 * This guard identifies ONLY the per-token deltas so they can be accumulated
 * into `streamingText` without duplication.
 */
export function isStreamingToken(chunk: Chunk): boolean {
  return (
    chunk.content_type === 'text' &&
    chunk.annotations[CHAT_IS_STREAMING] === true &&
    chunk.annotations[CHAT_STREAM_COMPLETE] !== true &&
    !(CHAT_MODEL in chunk.annotations)
  );
}

/** True once the stream is complete (the `stream_complete` chunk arrived). */
export function isStreamComplete(chunk: Chunk): boolean {
  return chunk.annotations[CHAT_STREAM_COMPLETE] === true;
}

/**
 * The durable "final response" chunk: a full assistant text chunk carrying
 * `chat.model`. It is NOT a transient token, and it is the chunk that survives
 * a reload — unlike the per-token deltas.
 */
export function isFinalResponse(chunk: Chunk): boolean {
  return (
    chunk.content_type === 'text' &&
    CHAT_IS_STREAMING in chunk.annotations &&
    CHAT_MODEL in chunk.annotations
  );
}

/**
 * The transient "stream start" marker: a null chunk with `chat.is_streaming`
 * (and assistant role) but no content. Used to anchor the live bubble.
 */
export function isStreamStart(chunk: Chunk): boolean {
  return (
    chunk.content_type === 'null' &&
    chunk.annotations[CHAT_IS_STREAMING] === true &&
    chunk.annotations[CHAT_STREAM_COMPLETE] !== true
  );
}

/**
 * A tombstone chunk carries the ids of transient chunks the producer has
 * retired (e.g. cafe-llm tombstones its token deltas once the final response is
 * published). Those ids must be hidden from the message list.
 */
export function tombstoneIds(chunk: Chunk): string[] | null {
  const ids = chunk.annotations[FLOW_TOMBSTONE];
  if (!Array.isArray(ids)) return null;
  return ids.filter((id): id is string => typeof id === 'string');
}

/**
 * Filter a raw chunk list for the "Raw" chat view: identical to
 * `chatMessagesFrom`, but keeps every non-chat chunk that is still meaningful
 * as a standalone row while hiding transient streaming token deltas (which
 * would otherwise render one bubble per token) and tombstoned chunks.
 */
export function rawViewChunks(chunks: Chunk[]): Chunk[] {
  const hidden = hiddenChunkIds(chunks);
  return chunks.filter(
    (c) =>
      !hidden.has(c.id) &&
      // Per-token deltas are transient text with chat.is_streaming and no
      // chat.model — never render them individually.
      !isStreamingToken(c) &&
      // Tombstone markers and delete signals themselves are not rows.
      tombstoneIds(c) === null &&
      deletionTarget(c) === null,
  );
}

/** A chat message candidate: a non-transient user or assistant text/bin chunk. */
export function isChatMessage(chunk: Chunk): boolean {
  // Skip transient chunks — streaming tokens, RPC envelopes, tombstones, etc.
  if (chunk.annotations['cafe.transient']) return false;
  // Skip per-token streaming deltas: they are accumulated into one live bubble
  // by `nextLiveStream` and must never render as individual messages. The
  // durable final response is a separate chunk (carries `chat.model`).
  if (isStreamingToken(chunk)) return false;
  if (
    chunk.content_type === 'text' &&
    (chunk.annotations['chat.role'] === 'user' ||
      chunk.annotations['chat.role'] === 'assistant')
  ) {
    return true;
  }
  if (
    (chunk.content_type === 'binary' || chunk.content_type === 'binary-ref') &&
    chunk.annotations['chat.role'] === 'assistant'
  ) {
    return true;
  }
  return false;
}

/**
 * The id a chunk retires via `flow.signal = "delete"`, if it is one. The bus
 * removes the target from history, but live clients also need to hide it so
 * the display updates immediately.
 */
export function deletionTarget(chunk: Chunk): string | null {
  if (chunk.annotations[FLOW_SIGNAL] !== 'delete') return null;
  const id = chunk.annotations[FLOW_TARGET_CHUNK_ID];
  return typeof id === 'string' ? id : null;
}

/** Collect every chunk id hidden by a tombstone or delete signal. */
export function hiddenChunkIds(chunks: Chunk[]): Set<string> {
  const hidden = new Set<string>();
  for (const chunk of chunks) {
    const ids = tombstoneIds(chunk);
    if (ids) for (const id of ids) hidden.add(id);
    const deleted = deletionTarget(chunk);
    if (deleted) hidden.add(deleted);
  }
  return hidden;
}

/**
 * Filter a raw chunk list down to the visible chat messages, applying
 * tombstones and delete signals: any chunk whose id a later tombstone or
 * delete retired is hidden. Durable final responses survive; transient
 * streaming rows (if they slipped through) are filtered by `isChatMessage`.
 */
export function chatMessagesFrom(chunks: Chunk[]): Chunk[] {
  const hidden = hiddenChunkIds(chunks);
  return chunks.filter((c) => isChatMessage(c) && !hidden.has(c.id));
}

/** A live streaming view: the accumulated text shown as one assistant bubble. */
export interface LiveStream {
  /** Stable React key — the stream-start chunk id when known, else a sentinel. */
  key: string;
  /** Accumulated token text so far. */
  content: string;
}

/**
 * Accumulate the per-token deltas of the currently-streaming assistant turn
 * into a single bubble.
 *
 * The bubble is anchored to the stream-start chunk id so React reuses one DOM
 * node across the whole stream (one bubble, not one per token). Returns null
 * when there is nothing to show yet.
 */
export function nextLiveStream(
  prev: LiveStream | null,
  chunk: Chunk,
): LiveStream | null {
  if (isStreamStart(chunk)) {
    return { key: chunk.id, content: prev?.content ?? '' };
  }
  if (isStreamingToken(chunk)) {
    const delta = typeof chunk.content === 'string' ? chunk.content : '';
    if (prev) return { key: prev.key, content: prev.content + delta };
    return { key: '__live_stream', content: delta };
  }
  return prev;
}

/** True once a chunk ends the live stream (tombstone or stream_complete). */
export function endsLiveStream(chunk: Chunk): boolean {
  return isStreamComplete(chunk) || tombstoneIds(chunk) !== null;
}


/**
 * Build the final assistant message content.
 *
 * The single source of truth is the accumulated per-token `streamingText`.
 * The `stream_complete` chunk (and the trailing full `response_chunk`) are
 * NEVER added again, so the text is not duplicated.
 */
export function buildFinalContent(streamingText: string, _finalChunk: Chunk): string {
  return streamingText;
}

/**
 * Pure reducer mirroring the QuickiesPanel streaming assembly.
 *
 * Feed it the full ordered sequence of chunks from the chat SSE and it returns
 * the accumulated `streamingText` plus the final assembled assistant message
 * content, with no duplication.
 */
export function assembleStream(chunks: Chunk[]): {
  streamingText: string;
  finalContent: string;
} {
  let streamingText = '';
  let finalContent = '';
  for (const chunk of chunks) {
    if (isStreamComplete(chunk)) {
      finalContent = buildFinalContent(streamingText, chunk);
    } else if (isStreamingToken(chunk)) {
      streamingText += typeof chunk.content === 'string' ? chunk.content : '';
    }
  }
  return { streamingText, finalContent };
}
