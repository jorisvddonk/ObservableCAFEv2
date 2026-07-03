import { useEffect, useState } from 'react';
import { listQuickies, createSession, streamChat } from 'cafe-web-sdk';
import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import type { Quickie, Chunk } from 'cafe-web-sdk';

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

export function QuickiesPanel() {
  const [quickies, setQuickies] = useState<Quickie[]>([]);
  const store = useSessionStore();
  const { switchSession, refresh } = useSessions();

  useEffect(() => {
    listQuickies()
      .then(setQuickies)
      .catch(() => {/* silently ignore if no quickies */});
  }, []);

  if (quickies.length === 0) return null;

  const launch = async (q: Quickie) => {
    const { id } = await createSession(q.agent_id);
    await refresh();
    await switchSession(id);

    if (q.starter_message) {
      store.setStreaming(true);
      const userChunk: Chunk = {
        id: uuid(),
        content_type: 'text',
        content: q.starter_message,
        data: null,
        mime_type: null,
        producer: 'com.nominal.cafe-web',
        annotations: { 'chat.role': 'user' },
        timestamp: Date.now(),
      };
      store.appendChunk(userChunk);

      await streamChat(
        id,
        q.starter_message,
        (chunk) => {
          if (chunk.content_type === 'text' && chunk.annotations['chat.is_streaming']) {
            store.appendStreamToken(typeof chunk.content === 'string' ? chunk.content : '');
          } else if (chunk.annotations['chat.stream_complete']) {
            const finalChunk: Chunk = {
              ...chunk,
              content_type: 'text',
              content: store.streamingText + (chunk.content ?? ''),
              annotations: { ...chunk.annotations, 'chat.role': 'assistant' },
            };
            store.finaliseStream(finalChunk);
          }
        },
        () => { store.setStreaming(false); store.clearStreamingText(); },
        (err) => { console.error(err); store.setStreaming(false); store.clearStreamingText(); },
      );
    }
  };

  return (
    <div style={{ padding: '8px 12px', borderTop: '1px solid #2a2a4a' }}>
      <div style={{ fontSize: 11, color: '#666', marginBottom: 6, textTransform: 'uppercase', letterSpacing: 1 }}>
        Quick start
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        {quickies.map((q) => (
          <button
            key={q.id}
            onClick={() => launch(q)}
            style={{
              background: '#0f3460',
              border: '1px solid #2a2a4a',
              borderRadius: 6,
              color: '#ccc',
              padding: '6px 10px',
              cursor: 'pointer',
              textAlign: 'left',
              fontSize: 12,
            }}
          >
            {q.emoji && <span style={{ marginRight: 6 }}>{q.emoji}</span>}
            {q.name}
          </button>
        ))}
      </div>
    </div>
  );
}
