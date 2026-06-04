import { useEffect, useRef, useState } from 'react';
import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import { streamChat } from '../api/chat';
import { openSessionStream } from '../api/stream';
import { Message } from './Message';
import type { Chunk } from '../types';

function uuid(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

/** True if a chunk should appear in the chat message list. */
function isChatMessage(chunk: Chunk): boolean {
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

export function ChatArea() {
  const store = useSessionStore();
  const { removeSession } = useSessions();
  const [input, setInput] = useState('');
  const bottomRef = useRef<HTMLDivElement>(null);
  // Track which chunk IDs are already in messages to avoid duplicates from the
  // persistent stream replaying history we already loaded.
  const seenIds = useRef<Set<string>>(new Set());
  const cleanupStream = useRef<(() => void) | null>(null);

  // Auto-scroll when messages change
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [store.messages]);

  // Open a persistent SSE stream for the active session.
  // This is the mechanism that delivers binary chunks (audio, images) that
  // arrive after the chat SSE closes at stream_complete.
  useEffect(() => {
    // Clean up previous stream
    cleanupStream.current?.();
    cleanupStream.current = null;
    seenIds.current = new Set(store.messages.map((c) => c.id));

    const sessionId = store.activeSessionId;
    if (!sessionId) return;

    const close = openSessionStream(
      sessionId,
      (chunk) => {
        // Always feed allChunks (chunk viewer)
        // Avoid double-adding chunks we loaded from history
        if (!seenIds.current.has(chunk.id)) {
          seenIds.current.add(chunk.id);
          useSessionStore.getState().appendChunk(chunk);
        }
      },
      (_count) => {
        // history replay complete — future chunks are live
      },
    );

    cleanupStream.current = close;
    return () => {
      close();
      cleanupStream.current = null;
    };
  }, [store.activeSessionId]);

  const send = async () => {
    const text = input.trim();
    const state = useSessionStore.getState();
    if (!text || state.streaming || !state.activeSessionId) return;
    setInput('');

    const userChunk: Chunk = {
      id: uuid(),
      content_type: 'text',
      content: text,
      data: null,
      mime_type: null,
      producer: 'com.nominal.cafe-web',
      annotations: { 'chat.role': 'user' },
      timestamp: Date.now(),
    };
    // Add locally — also register in seenIds so the stream doesn't double-add
    seenIds.current.add(userChunk.id);
    state.appendChunk(userChunk);
    state.setStreaming(true);

    await streamChat(
      state.activeSessionId,
      text,
      (chunk) => {
        // Chat SSE delivers text chunks; register them in seenIds so the
        // persistent stream doesn't duplicate them.
        if (!seenIds.current.has(chunk.id)) {
          seenIds.current.add(chunk.id);
          useSessionStore.getState().appendChunk(chunk);
        }
      },
      () => {
        useSessionStore.getState().setStreaming(false);
      },
      (err) => {
        console.error('[ChatArea] onError', err);
        useSessionStore.getState().setStreaming(false);
      },
    );
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  if (!store.activeSessionId) {
    return (
      <div
        style={{
          flex: 1,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          color: '#555',
          fontSize: 15,
        }}
      >
        Select a session or create a new one
      </div>
    );
  }

  // Filter messages for the chat display
  const displayMessages = store.messages.filter(isChatMessage);

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
      {/* Header */}
      <div
        style={{
          padding: '10px 16px',
          borderBottom: '1px solid #2a2a4a',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          background: '#16213e',
        }}
      >
        <span style={{ fontWeight: 600, color: '#4fc3f7', fontSize: 14 }}>
          {store.sessions.find((s) => s.session_id === store.activeSessionId)
            ?.display_name ?? store.activeSessionId?.slice(0, 12) + '…'}
        </span>
        <button
          onClick={() => store.activeSessionId && removeSession(store.activeSessionId)}
          style={{
            background: 'transparent',
            border: '1px solid #444',
            color: '#888',
            borderRadius: 4,
            padding: '2px 8px',
            cursor: 'pointer',
            fontSize: 12,
          }}
        >
          Delete
        </button>
      </div>

      {/* Messages */}
      <div
        style={{
          flex: 1,
          overflowY: 'auto',
          padding: '16px',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        {displayMessages.map((chunk) => (
          <Message key={chunk.id} chunk={chunk} />
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Input */}
      <div
        style={{
          padding: '12px 16px',
          borderTop: '1px solid #2a2a4a',
          background: '#16213e',
          display: 'flex',
          gap: 8,
        }}
      >
        <textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={
            store.streaming
              ? 'Waiting for response…'
              : 'Type a message… (Enter to send, Shift+Enter for newline)'
          }
          disabled={store.streaming}
          rows={1}
          style={{
            flex: 1,
            background: '#0f3460',
            border: '1px solid #2a2a4a',
            borderRadius: 6,
            color: '#e0e0e0',
            padding: '8px 12px',
            fontSize: 14,
            resize: 'none',
            outline: 'none',
            fontFamily: 'inherit',
          }}
        />
        <button
          onClick={send}
          disabled={store.streaming || !input.trim()}
          style={{
            background: '#4fc3f7',
            color: '#1a1a2e',
            border: 'none',
            borderRadius: 6,
            padding: '8px 16px',
            fontWeight: 600,
            cursor: store.streaming ? 'not-allowed' : 'pointer',
            opacity: store.streaming || !input.trim() ? 0.5 : 1,
            fontSize: 14,
          }}
        >
          Send
        </button>
      </div>
    </div>
  );
}
