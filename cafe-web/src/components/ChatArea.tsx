import { useEffect, useRef, useState } from 'react';
import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import { streamChat, openSessionStream, publishChunk } from 'cafe-web-sdk';
import { Message } from './Message';
import type { Chunk } from 'cafe-web-sdk';

/** True if a chunk should appear in the chat message list. */
function isChatMessage(chunk: Chunk): boolean {
  // Skip transient chunks — streaming tokens, RPC envelopes, etc.
  if (chunk.annotations['cafe.transient']) return false;
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

    // /addchunk <type> [key=value...] — publish a chunk directly, bypassing LLM
    if (text.startsWith('/addchunk ')) {
      const rest = text.slice('/addchunk '.length);
      const space = rest.indexOf(' ');
      const contentType = space === -1 ? rest : rest.slice(0, space);
      const annotStr = space === -1 ? '' : rest.slice(space + 1);
      const annotations: Record<string, unknown> = {};
      const re = /(\S+)=("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\S+)/g;
      let m: RegExpExecArray | null;
      while ((m = re.exec(annotStr)) !== null) {
        const k = m[1];
        let v = m[2];
        if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) {
          v = v.slice(1, -1).replace(/\\(.)/g, '$1');
        }
        annotations[k] = v === 'true' ? true : v === 'false' ? false : v;
      }
      try {
        await publishChunk(state.activeSessionId, contentType, annotations);
      } catch (err) {
        console.error('[ChatArea] publishChunk error', err);
      }
      return;
    }

    // /voice <name> — shorthand for changing TTS voice profile
    if (text.startsWith('/voice ')) {
      const profile = text.slice('/voice '.length).trim();
      if (profile) {
        try {
          await publishChunk(state.activeSessionId, 'null', {
            'config.type': 'runtime',
            'config.tts.profile': profile,
          });
        } catch (err) {
          console.error('[ChatArea] /voice error', err);
        }
      }
      return;
    }

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
  const displayMessages = store.showAllChunks
    ? store.allChunks
    : store.messages.filter(isChatMessage);

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
            ?.display_name ?? store.activeSessionId}
        </span>
        <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <button
            onClick={store.toggleShowAllChunks}
            style={{
              background: store.showAllChunks ? '#4fc3f7' : '#0f3460',
              color: store.showAllChunks ? '#1a1a2e' : '#888',
              border: '1px solid #444',
              borderRadius: 4,
              padding: '2px 8px',
              cursor: 'pointer',
              fontSize: 12,
              fontWeight: store.showAllChunks ? 600 : 400,
            }}
          >
            Raw
          </button>
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
              ? 'Responding…'
              : 'Type a message… (Enter to send, Shift+Enter for newline)'
          }
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
