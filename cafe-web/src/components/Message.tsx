import { useSessionStore } from '../store/sessions';
import { getBinaryUrl, asBinaryRef, chunkMimeType, isMediaChunk } from 'cafe-web-sdk';
import type { Chunk } from 'cafe-web-sdk';

interface Props {
  chunk: Chunk;
}

export function Message({ chunk }: Props) {
  if (isMediaChunk(chunk)) {
    return <MediaMessage chunk={chunk} />;
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
        {typeof chunk.content === 'string' ? chunk.content : null}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Media (binary / binary-ref)
// ---------------------------------------------------------------------------

function MediaMessage({ chunk }: { chunk: Chunk }) {
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const mime = chunkMimeType(chunk);
  const ref = asBinaryRef(chunk);

  // Resolve the media src:
  //  - binary-ref → URL endpoint (fetched + cached by browser)
  //  - full binary → inline data URI
  const src = ref
    ? getBinaryUrl(activeSessionId ?? '', ref.chunk_id)
    : chunk.data
      ? `data:${mime};base64,${chunk.data}`
      : null;

  const byteSize = ref
    ? ref.byte_size
    : chunk.data
      ? Math.round((chunk.data.length * 3) / 4 / 1024)
      : null;

  if (!src) {
    return (
      <div style={{ color: '#666', fontSize: 12, marginBottom: 12 }}>
        [empty binary chunk]
      </div>
    );
  }

  if (mime?.startsWith('audio/')) {
    return (
      <div
        className="message message--binary message--audio"
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'flex-start',
          marginBottom: 12,
        }}
      >
        <span style={{ fontSize: 11, color: '#888', marginBottom: 4 }}>
          assistant · audio{byteSize ? ` · ${byteSize} KB` : ''}
        </span>
        <audio
          controls
          src={src}
          style={{ maxWidth: '100%', borderRadius: 6 }}
        />
      </div>
    );
  }

  if (mime?.startsWith('image/')) {
    return (
      <div
        className="message message--binary message--image"
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'flex-start',
          marginBottom: 12,
        }}
      >
        <span style={{ fontSize: 11, color: '#888', marginBottom: 4 }}>
          assistant · image{byteSize ? ` · ${byteSize} KB` : ''}
        </span>
        <img
          src={src}
          alt="Image from assistant"
          style={{ maxWidth: '80%', borderRadius: 8, display: 'block' }}
        />
      </div>
    );
  }

  // Unknown binary type — show badge
  return (
    <div
      className="message message--binary"
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-start',
        marginBottom: 12,
      }}
    >
      <span style={{ fontSize: 11, color: '#888', marginBottom: 4 }}>assistant</span>
      <a
        href={src}
        download
        style={{
          background: '#0f3460',
          borderRadius: 8,
          padding: '6px 12px',
          fontSize: 12,
          color: '#4fc3f7',
          textDecoration: 'none',
        }}
      >
        📎 {mime ?? 'binary'}{byteSize ? ` (${byteSize} KB)` : ''}
      </a>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Trust prompt
// ---------------------------------------------------------------------------

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
