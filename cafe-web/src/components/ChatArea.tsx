import { useEffect, useRef, useState } from 'react';
import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import { streamChat } from '../api/chat';
import { Message } from './Message';
import { StreamingIndicator } from './StreamingIndicator';
import type { Chunk } from '../types';

export function ChatArea() {
  const store = useSessionStore();
  const { removeSession } = useSessions();
  const [input, setInput] = useState('');
  const bottomRef = useRef<HTMLDivElement>(null);

  // Scroll to bottom when messages change
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [store.messages, store.streamingText]);

  const send = async () => {
    const text = input.trim();
    if (!text || store.streaming || !store.activeSessionId) return;
    setInput('');

    // Optimistically add user message
    const userChunk: Chunk = {
      id: crypto.randomUUID(),
      content_type: 'text',
      content: text,
      data: null,
      mime_type: null,
      producer: 'com.nominal.cafe-web',
      annotations: { 'chat.role': 'user' },
      timestamp: Date.now(),
    };
    store.appendChunk(userChunk);
    store.setStreaming(true);

    await streamChat(
      store.activeSessionId,
      text,
      (chunk) => {
        if (chunk.content_type === 'text' && chunk.annotations['chat.is_streaming']) {
          store.appendStreamToken(chunk.content ?? '');
        } else if (chunk.annotations['chat.stream_complete']) {
          // Build final assistant chunk from accumulated text
          const finalChunk: Chunk = {
            ...chunk,
            content_type: 'text',
            content: store.streamingText + (chunk.content ?? ''),
            annotations: { ...chunk.annotations, 'chat.role': 'assistant' },
          };
          store.finaliseStream(finalChunk);
        }
      },
      () => {
        store.setStreaming(false);
        store.clearStreamingText();
      },
      (err) => {
        console.error('Stream error:', err);
        store.setStreaming(false);
        store.clearStreamingText();
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
        {store.messages.map((chunk) => (
          <Message key={chunk.id} chunk={chunk} />
        ))}
        <StreamingIndicator text={store.streamingText} />
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
          placeholder={store.streaming ? 'Waiting for response…' : 'Type a message… (Enter to send, Shift+Enter for newline)'}
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
