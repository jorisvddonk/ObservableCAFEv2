import { describe, it, expect } from 'vitest';
import type { Chunk } from 'cafe-web-sdk';
import { assembleStream, isStreamingToken, buildFinalContent } from './streaming';

const FULL = 'The quick brown fox jumps over the lazy dog.';

// Per-token deltas as emitted by cafe-llm/evaluator.rs (transient, no chat.model)
const deltas: Chunk[] = [
  'The quick ',
  'brown fox ',
  'jumps over ',
  'the lazy dog.',
].map((content, i) => ({
  id: `delta-${i}`,
  content_type: 'text' as const,
  content,
  data: null,
  mime_type: null,
  producer: 'com.nominal.cafe-llm',
  annotations: { 'chat.role': 'assistant', 'chat.is_streaming': true },
  timestamp: 0,
}));

// The trailing full response_chunk: ALSO has chat.is_streaming=true, plus chat.model
const fullResponse: Chunk = {
  id: 'full-response',
  content_type: 'text',
  content: FULL,
  data: null,
  mime_type: null,
  producer: 'com.nominal.cafe-llm',
  annotations: {
    'chat.role': 'assistant',
    'chat.is_streaming': true,
    'chat.model': 'llama-3.2',
  },
  timestamp: 0,
};

// The stream_complete chunk (null content)
const done: Chunk = {
  id: 'done',
  content_type: 'null',
  content: null,
  data: null,
  mime_type: null,
  producer: 'com.nominal.cafe-llm',
  annotations: { 'chat.role': 'assistant', 'chat.stream_complete': true, 'chat.finish_reason': 'stop' },
  timestamp: 0,
};

describe('QuickiesPanel streaming assembly', () => {
  it('does NOT treat the full response_chunk as a streaming token', () => {
    expect(isStreamingToken(deltas[0])).toBe(true);
    expect(isStreamingToken(fullResponse)).toBe(false);
  });

  it('assembles the assistant message without duplication', () => {
    const { streamingText, finalContent } = assembleStream([...deltas, fullResponse, done]);
    expect(streamingText).toBe(FULL);
    expect(finalContent).toBe(FULL);
    // Critical: must equal the real model text, NOT ~2x.
    expect(finalContent.length).toBe(FULL.length);
  });

  it('buildFinalContent ignores the trailing full content', () => {
    expect(buildFinalContent(FULL, fullResponse)).toBe(FULL);
  });
});
