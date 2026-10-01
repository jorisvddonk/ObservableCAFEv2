import { describe, it, expect } from 'vitest';
import type { Chunk } from 'cafe-web-sdk';
import {
  assembleStream,
  buildFinalContent,
  chatMessagesFrom,
  endsLiveStream,
  isStreamingToken,
  nextLiveStream,
  rawViewChunks,
  tombstoneIds,
} from './streaming';
import type { LiveStream } from './streaming';

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

// The stream-start marker emitted before the first token (transient, null).
const start: Chunk = {
  id: 'stream-start',
  content_type: 'null',
  content: null,
  data: null,
  mime_type: null,
  producer: 'com.nominal.cafe-llm',
  annotations: { 'chat.role': 'assistant', 'chat.is_streaming': true },
  timestamp: 0,
};

// The tombstone cafe-llm publishes once the final response is out.
const tombstone: Chunk = {
  id: 'tombstone',
  content_type: 'null',
  content: null,
  data: null,
  mime_type: null,
  producer: 'com.nominal.cafe-llm',
  annotations: { 'cafe.flow.tombstone': deltas.map((d) => d.id) },
  timestamp: 0,
};

describe('live stream single bubble', () => {
  it('anchors the bubble to the stream-start chunk and appends deltas', () => {
    let live = nextLiveStream(null, start);
    expect(live).toEqual({ key: 'stream-start', content: '' });
    for (const d of deltas) live = nextLiveStream(live, d);
    // ONE bubble whose key never changed — React reuses the same DOM node.
    expect(live).toEqual({ key: 'stream-start', content: FULL });
  });

  it('accumulates deltas even without an explicit start marker', () => {
    let live = nextLiveStream(null, deltas[0]);
    expect(live?.key).toBe('__live_stream');
    live = nextLiveStream(live, deltas[1]);
    expect(live).toEqual({ key: '__live_stream', content: 'The quick brown fox ' });
  });

  it('ignores the final response chunk (not a token)', () => {
    let live = nextLiveStream({ key: 'stream-start', content: FULL }, fullResponse);
    expect(live).toEqual({ key: 'stream-start', content: FULL });
  });

  it('flags tombstone and stream_complete as stream enders', () => {
    expect(endsLiveStream(tombstone)).toBe(true);
    expect(endsLiveStream(done)).toBe(true);
    expect(endsLiveStream(deltas[0])).toBe(false);
    expect(endsLiveStream(start)).toBe(false);
  });

  it('reads tombstoned ids', () => {
    expect(tombstoneIds(tombstone)).toEqual(deltas.map((d) => d.id));
    expect(tombstoneIds(deltas[0])).toBeNull();
  });
});

describe('chatMessagesFrom', () => {
  it('hides tombstoned chunks while keeping the durable final response', () => {
    const history = [start, ...deltas, fullResponse, tombstone, done];
    const visible = chatMessagesFrom(history);
    const ids = visible.map((c) => c.id);
    // The tombstone retires the token deltas; the durable final response stays.
    expect(ids).toEqual(['full-response']);
  });

  it('keeps user and durable assistant text when nothing is tombstoned', () => {
    const user: Chunk = {
      id: 'u1',
      content_type: 'text',
      content: 'hi',
      data: null,
      mime_type: null,
      producer: 'com.nominal.cafe-server',
      annotations: { 'chat.role': 'user' },
      timestamp: 0,
    };
    // Transient token deltas must NOT render as individual messages even with
    // no tombstone (the real cafe-llm stream emits none): the durable final
    // response is the single assistant bubble.
    const visible = chatMessagesFrom([user, ...deltas, fullResponse, done]);
    expect(visible.map((c) => c.id)).toEqual(['u1', 'full-response']);
  });

  it('drops transient token deltas even without a tombstone', () => {
    const transientDelta: Chunk = {
      ...deltas[0],
      annotations: { ...deltas[0].annotations, 'cafe.transient': true },
    };
    const visible = chatMessagesFrom([transientDelta, fullResponse]);
    expect(visible.map((c) => c.id)).toEqual(['full-response']);
  });
});

describe('rawViewChunks (Raw mode chat list)', () => {
  it('never renders per-token deltas as individual rows', () => {
    const history = [start, ...deltas, fullResponse, tombstone, done];
    const raw = rawViewChunks(history);
    const ids = raw.map((c) => c.id);
    // The per-token deltas and the tombstone are hidden; the durable final
    // response remains. This is what stops the "bazillion bubbles" in Raw mode.
    expect(ids).not.toContain('delta-0');
    expect(ids).not.toContain('tombstone');
    expect(ids).toContain('full-response');
  });
});

describe('dual-channel ingestion (chat SSE + persistent stream)', () => {
  // Both channels deliver the SAME chunk object. Accumulating without an id
  // guard double-appends every token ("DoDoing…").
  it('appends each token exactly once when delivered twice', () => {
    const seen = new Set<string>();
    let live: LiveStream | null = null;
    const ingest = (state: LiveStream | null, c: Chunk): LiveStream | null => {
      if (seen.has(c.id)) return state;
      seen.add(c.id);
      return nextLiveStream(state, c);
    };
    for (const d of deltas) {
      live = ingest(live, d); // chat SSE
      live = ingest(live, d); // persistent stream, same chunk
    }
    expect(live?.content).toBe(FULL);
    expect(live?.content).not.toBe('The quick The quick brown fox brown fox ');
  });
});
