import { useState, useRef, useEffect } from 'react';
import { useSessionStore } from '../store/sessions';
import type { Chunk } from '../types';

// ── colour coding per content type / role ────────────────────────────────────

const ROLE_COLORS: Record<string, string> = {
  user:      '#4fc3f7',
  assistant: '#81c784',
  system:    '#ce93d8',
};

function chunkColor(chunk: Chunk): string {
  if (chunk.content_type === 'null') {
    if (chunk.annotations['jsonrpc.request'])  return '#ffb74d';   // orange — RPC request
    if (chunk.annotations['jsonrpc.response']) return '#4db6ac';   // teal   — RPC response
    if (chunk.annotations['config.type'])      return '#9575cd';   // purple — config
    if (chunk.annotations['chat.stream_complete']) return '#546e7a'; // slate — stream done
    return '#607d8b';                                               // grey   — other null
  }
  if (chunk.content_type === 'binary') return '#f06292';           // pink   — audio/image
  const role = chunk.annotations['chat.role'] as string | undefined;
  return role ? (ROLE_COLORS[role] ?? '#aaa') : '#aaa';
}

function contentTypeLabel(chunk: Chunk): string {
  if (chunk.content_type === 'null') {
    if (chunk.annotations['jsonrpc.request'])      return 'RPC req';
    if (chunk.annotations['jsonrpc.response'])     return 'RPC resp';
    if (chunk.annotations['config.type'])          return 'config';
    if (chunk.annotations['chat.stream_complete']) return 'done';
    return 'null';
  }
  if (chunk.content_type === 'binary') return chunk.mime_type?.split('/')[1] ?? 'binary';
  const role = chunk.annotations['chat.role'] as string | undefined;
  return role ?? 'text';
}

// ── single row ────────────────────────────────────────────────────────────────

function ChunkRow({
  chunk,
  selected,
  onSelect,
}: {
  chunk: Chunk;
  selected: boolean;
  onSelect: () => void;
}) {
  const color = chunkColor(chunk);
  const label = contentTypeLabel(chunk);
  const ts = new Date(chunk.timestamp).toISOString().slice(11, 23); // HH:MM:SS.mmm
  const producer = chunk.producer.replace('com.nominal.', '');

  let preview = '';
  if (chunk.content_type === 'text') {
    preview = (chunk.content ?? '').slice(0, 80);
  } else if (chunk.content_type === 'null') {
    if (chunk.annotations['jsonrpc.request']) {
      const r = chunk.annotations['jsonrpc.request'] as { method?: string; id?: string };
      preview = `${r.method ?? '?'} id=${(r.id ?? '').slice(0, 8)}`;
    } else if (chunk.annotations['jsonrpc.response']) {
      const r = chunk.annotations['jsonrpc.response'] as { id?: string; result?: unknown; error?: unknown };
      preview = r.error
        ? `ERR id=${(r.id ?? '').slice(0, 8)}`
        : `OK  id=${(r.id ?? '').slice(0, 8)}`;
    } else if (chunk.annotations['chat.stream_complete']) {
      const reason = chunk.annotations['chat.finish_reason'] as string | undefined;
      preview = reason ? `finish_reason=${reason}` : '';
    }
  } else if (chunk.content_type === 'binary') {
    preview = `${chunk.mime_type ?? 'unknown'} ${
      chunk.data ? Math.round((chunk.data.length * 3) / 4 / 1024) + ' KB' : ''
    }`;
  }

  return (
    <button
      onClick={onSelect}
      style={{
        display: 'grid',
        gridTemplateColumns: '80px 70px 130px 1fr',
        gap: '0 8px',
        alignItems: 'center',
        width: '100%',
        padding: '4px 8px',
        background: selected ? '#1e2a45' : 'transparent',
        border: 'none',
        borderLeft: selected ? `3px solid ${color}` : '3px solid transparent',
        cursor: 'pointer',
        textAlign: 'left',
        fontFamily: 'monospace',
        fontSize: 11,
        color: '#ccc',
        borderBottom: '1px solid #1a1a30',
      }}
    >
      <span style={{ color: '#666' }}>{ts}</span>
      <span
        style={{
          background: color + '22',
          color: color,
          borderRadius: 3,
          padding: '1px 4px',
          fontWeight: 600,
          textAlign: 'center',
        }}
      >
        {label}
      </span>
      <span style={{ color: '#888', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {producer}
      </span>
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: '#aaa' }}>
        {preview}
      </span>
    </button>
  );
}

// ── detail pane ───────────────────────────────────────────────────────────────

function ChunkDetail({ chunk }: { chunk: Chunk }) {
  const [copied, setCopied] = useState(false);

  const json = JSON.stringify(
    {
      ...chunk,
      // Don't render large base64 blobs inline — replace with placeholder
      data: chunk.data ? `<base64 ${Math.round((chunk.data.length * 3) / 4 / 1024)} KB>` : null,
    },
    null,
    2,
  );

  const copy = () => {
    navigator.clipboard.writeText(JSON.stringify(chunk, null, 2)).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  const color = chunkColor(chunk);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      {/* Header bar */}
      <div
        style={{
          padding: '6px 10px',
          background: '#16213e',
          borderBottom: '1px solid #2a2a4a',
          display: 'flex',
          alignItems: 'center',
          gap: 8,
        }}
      >
        <span
          style={{
            background: color + '22',
            color: color,
            borderRadius: 3,
            padding: '1px 6px',
            fontWeight: 700,
            fontSize: 11,
            fontFamily: 'monospace',
          }}
        >
          {contentTypeLabel(chunk)}
        </span>
        <span style={{ fontFamily: 'monospace', fontSize: 11, color: '#666', flex: 1 }}>
          {chunk.id}
        </span>
        <button
          onClick={copy}
          style={{
            background: '#0f3460',
            border: '1px solid #2a2a4a',
            color: copied ? '#81c784' : '#aaa',
            borderRadius: 4,
            padding: '2px 8px',
            fontSize: 11,
            cursor: 'pointer',
          }}
        >
          {copied ? '✓ Copied' : 'Copy JSON'}
        </button>
      </div>

      {/* Audio preview for binary audio chunks */}
      {chunk.content_type === 'binary' && chunk.mime_type?.startsWith('audio/') && chunk.data && (
        <div style={{ padding: '8px 10px', borderBottom: '1px solid #1a1a30' }}>
          <audio
            controls
            src={`data:${chunk.mime_type};base64,${chunk.data}`}
            style={{ width: '100%' }}
          />
        </div>
      )}

      {/* JSON body */}
      <pre
        style={{
          flex: 1,
          margin: 0,
          padding: '10px',
          overflowY: 'auto',
          fontFamily: 'monospace',
          fontSize: 11,
          color: '#c9d1d9',
          background: '#0d1117',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-all',
        }}
      >
        {json}
      </pre>
    </div>
  );
}

// ── filter bar ────────────────────────────────────────────────────────────────

type FilterType = 'all' | 'text' | 'null' | 'binary' | 'rpc' | 'config';

function applyFilter(chunks: Chunk[], filter: FilterType, search: string): Chunk[] {
  let result = chunks;

  if (filter === 'text')   result = result.filter((c) => c.content_type === 'text');
  if (filter === 'binary') result = result.filter((c) => c.content_type === 'binary');
  if (filter === 'null')   result = result.filter((c) => c.content_type === 'null');
  if (filter === 'rpc')    result = result.filter(
    (c) => c.annotations['jsonrpc.request'] || c.annotations['jsonrpc.response'],
  );
  if (filter === 'config') result = result.filter((c) => c.annotations['config.type']);

  if (search.trim()) {
    const q = search.toLowerCase();
    result = result.filter((c) => {
      return (
        c.id.includes(q) ||
        c.producer.toLowerCase().includes(q) ||
        (c.content ?? '').toLowerCase().includes(q) ||
        JSON.stringify(c.annotations).toLowerCase().includes(q)
      );
    });
  }

  return result;
}

// ── main panel ────────────────────────────────────────────────────────────────

export function ChunkViewer() {
  const { allChunks, chunkViewerOpen, toggleChunkViewer, activeSessionId } = useSessionStore();
  const [selected, setSelected] = useState<Chunk | null>(null);
  const [filter, setFilter] = useState<FilterType>('all');
  const [search, setSearch] = useState('');
  const listRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  // Auto-scroll list to bottom when new chunks arrive
  useEffect(() => {
    if (autoScroll && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [allChunks, autoScroll]);

  // Reset selection when session changes
  useEffect(() => {
    setSelected(null);
  }, [activeSessionId]);

  if (!chunkViewerOpen) return null;

  const visible = applyFilter(allChunks, filter, search);

  const FILTER_BTNS: { key: FilterType; label: string }[] = [
    { key: 'all',    label: 'All' },
    { key: 'text',   label: 'Text' },
    { key: 'null',   label: 'Null' },
    { key: 'binary', label: 'Binary' },
    { key: 'rpc',    label: 'RPC' },
    { key: 'config', label: 'Config' },
  ];

  return (
    <div
      style={{
        position: 'fixed',
        bottom: 0,
        left: 0,
        right: 0,
        height: '40vh',
        background: '#0d1117',
        borderTop: '2px solid #2a2a4a',
        display: 'flex',
        flexDirection: 'column',
        zIndex: 1000,
        fontFamily: 'monospace',
      }}
    >
      {/* Toolbar */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          padding: '4px 8px',
          background: '#16213e',
          borderBottom: '1px solid #2a2a4a',
          flexShrink: 0,
        }}
      >
        <span style={{ fontWeight: 700, fontSize: 12, color: '#4fc3f7', marginRight: 4 }}>
          Chunk Viewer
        </span>

        {/* Filter buttons */}
        {FILTER_BTNS.map(({ key, label }) => (
          <button
            key={key}
            onClick={() => setFilter(key)}
            style={{
              background: filter === key ? '#4fc3f7' : '#0f3460',
              color: filter === key ? '#1a1a2e' : '#aaa',
              border: '1px solid #2a2a4a',
              borderRadius: 4,
              padding: '2px 7px',
              fontSize: 11,
              cursor: 'pointer',
              fontWeight: filter === key ? 700 : 400,
            }}
          >
            {label}
          </button>
        ))}

        {/* Search */}
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search…"
          style={{
            flex: 1,
            background: '#0f3460',
            border: '1px solid #2a2a4a',
            borderRadius: 4,
            color: '#e0e0e0',
            padding: '2px 8px',
            fontSize: 11,
            outline: 'none',
          }}
        />

        {/* Auto-scroll toggle */}
        <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 11, color: '#888', cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={autoScroll}
            onChange={(e) => setAutoScroll(e.target.checked)}
            style={{ cursor: 'pointer' }}
          />
          Live
        </label>

        <span style={{ fontSize: 11, color: '#555', marginLeft: 4 }}>
          {visible.length}/{allChunks.length}
        </span>

        {/* Close */}
        <button
          onClick={toggleChunkViewer}
          style={{
            background: 'transparent',
            border: 'none',
            color: '#666',
            fontSize: 16,
            cursor: 'pointer',
            lineHeight: 1,
            padding: '0 4px',
          }}
          title="Close chunk viewer"
        >
          ✕
        </button>
      </div>

      {/* Body: list + detail */}
      <div style={{ display: 'flex', flex: 1, minHeight: 0 }}>
        {/* Chunk list */}
        <div
          ref={listRef}
          onScroll={() => {
            if (!listRef.current) return;
            const { scrollTop, scrollHeight, clientHeight } = listRef.current;
            setAutoScroll(scrollTop + clientHeight >= scrollHeight - 10);
          }}
          style={{
            width: selected ? '55%' : '100%',
            overflowY: 'auto',
            borderRight: selected ? '1px solid #2a2a4a' : 'none',
          }}
        >
          {/* Column headers */}
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: '80px 70px 130px 1fr',
              gap: '0 8px',
              padding: '3px 8px',
              background: '#16213e',
              borderBottom: '1px solid #1a1a30',
              position: 'sticky',
              top: 0,
            }}
          >
            {['Time', 'Type', 'Producer', 'Preview'].map((h) => (
              <span key={h} style={{ fontSize: 10, color: '#555', fontWeight: 700 }}>
                {h}
              </span>
            ))}
          </div>

          {visible.length === 0 ? (
            <div style={{ padding: 16, color: '#444', fontSize: 12 }}>No chunks match.</div>
          ) : (
            visible.map((c) => (
              <ChunkRow
                key={c.id}
                chunk={c}
                selected={selected?.id === c.id}
                onSelect={() => setSelected(selected?.id === c.id ? null : c)}
              />
            ))
          )}
        </div>

        {/* Detail pane */}
        {selected && (
          <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
            <ChunkDetail chunk={selected} />
          </div>
        )}
      </div>
    </div>
  );
}
