import type { Chunk } from '../types';

interface Props {
  chunk: Chunk;
}

export function Message({ chunk }: Props) {
  if (chunk.content_type === 'binary') {
    if (chunk.mime_type?.startsWith('image/')) {
      return (
        <div className="message message--binary">
          <img
            src={`data:${chunk.mime_type};base64,${chunk.data}`}
            alt="Image from assistant"
            style={{ maxWidth: '100%', borderRadius: 8 }}
          />
        </div>
      );
    }
    if (chunk.mime_type?.startsWith('audio/')) {
      return (
        <div className="message message--binary">
          <audio
            controls
            src={`data:${chunk.mime_type};base64,${chunk.data}`}
          />
        </div>
      );
    }
    return (
      <div className="message message--binary">
        <span style={{ color: '#888' }}>[Binary: {chunk.mime_type ?? 'unknown'}]</span>
      </div>
    );
  }

  if (chunk.content_type === 'null') {
    const trustLevel = chunk.annotations['security.trust-level'] as
      | { trusted: boolean }
      | undefined;
    if (trustLevel?.trusted === false) {
      return <TrustPrompt chunk={chunk} />;
    }
    return null;
  }

  const role = chunk.annotations['chat.role'] as string | undefined;
  const isUser = role === 'user';

  return (
    <div
      className={`message message--${role ?? 'system'}`}
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: isUser ? 'flex-end' : 'flex-start',
        marginBottom: 12,
      }}
    >
      <span
        style={{
          fontSize: 11,
          color: '#888',
          marginBottom: 2,
          textTransform: 'capitalize',
        }}
      >
        {role ?? 'system'}
      </span>
      <div
        style={{
          background: isUser ? '#16213e' : '#0f3460',
          borderRadius: 8,
          padding: '8px 12px',
          maxWidth: '80%',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
          lineHeight: 1.5,
        }}
      >
        {chunk.content}
      </div>
    </div>
  );
}

function TrustPrompt({ chunk }: { chunk: Chunk }) {
  return (
    <div
      style={{
        background: '#2a1a0e',
        border: '1px solid #8b4513',
        borderRadius: 8,
        padding: '8px 12px',
        marginBottom: 12,
        fontSize: 13,
      }}
    >
      <strong style={{ color: '#ffa500' }}>⚠ Untrusted content</strong>
      <p style={{ marginTop: 4, color: '#ccc' }}>
        Web content from{' '}
        <em>{String(chunk.annotations['web.source_url'] ?? 'unknown source')}</em>{' '}
        is waiting for your approval before the LLM can see it.
      </p>
      <p style={{ marginTop: 4, fontSize: 11, color: '#888' }}>
        Chunk ID: {chunk.id}
      </p>
    </div>
  );
}
