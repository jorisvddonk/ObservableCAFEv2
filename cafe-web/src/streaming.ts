import type { Chunk } from 'cafe-web-sdk';
import { CHAT_IS_STREAMING, CHAT_MODEL, CHAT_STREAM_COMPLETE } from 'cafe-web-sdk';

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
