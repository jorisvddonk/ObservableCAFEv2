import { useEffect, useMemo, useRef, useState } from 'react';
import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import { useModels } from '../hooks/useModels';
import { streamChat, openSessionStream, publishChunk, backendForModel } from 'cafe-web-sdk';
import { Message } from './Message';
import {
  chatMessagesFrom,
  endsLiveStream,
  nextLiveStream,
  rawViewChunks,
  tombstoneIds,
} from '../streaming';
import type { Chunk } from 'cafe-web-sdk';

export function ChatArea() {
  const store = useSessionStore();
  const { removeSession, duplicateSession } = useSessions();
  const { catalog } = useModels();
  const [input, setInput] = useState('');
  const [forking, setForking] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);
  // Track which chunk IDs are already in messages to avoid duplicates from the
  // persistent stream replaying history we already loaded.
  const seenIds = useRef<Set<string>>(new Set());
  // Chunk IDs already folded into the live streaming bubble. The chat SSE and
  // the persistent session stream deliver the SAME chunk to both callbacks, so
  // without this guard each token would be appended to the bubble twice
  // (e.g. "DoDoing…").
  const liveSeenIds = useRef<Set<string>>(new Set());
  const cleanupStream = useRef<(() => void) | null>(null);

  // Fold a chunk into the single live streaming bubble. Guarded by chunk id so
  // the chat SSE and the persistent session stream (which both deliver the same
  // chunk) contribute each token exactly once.
  const ingestLive = (state: typeof store, chunk: Chunk) => {
    if (liveSeenIds.current.has(chunk.id)) return;
    liveSeenIds.current.add(chunk.id);
    // A tombstone or stream_complete retires the live view; the durable final
    // response chunk is already in `messages` — never render both.
    if (tombstoneIds(chunk) !== null || endsLiveStream(chunk)) {
      state.setLiveStream(null);
      return;
    }
    const live = nextLiveStream(state.liveStream, chunk);
    if (live) state.setLiveStream(live);
  };

  // Auto-scroll when messages change
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [store.messages]);

  // `/model` autocomplete: when the input is a model command, suggest matching
  // models from the catalog (each labelled with its serving backend).
  const modelQuery = useMemo(() => {
    const m = input.match(/^\/model(?:\s+(.*))?$/);
    return m ? (m[1] ?? '') : null;
  }, [input]);

  const suggestions = useMemo(() => {
    if (modelQuery === null) return [];
    const q = modelQuery.toLowerCase();
    const rows = catalog.backends.flatMap((b) =>
      b.models.map((model) => ({ model, backend: b.backend })),
    );
    // De-duplicate by model, keeping the first backend that serves it.
    const byModel = new Map<string, { model: string; backend: string }>();
    for (const r of rows) if (!byModel.has(r.model)) byModel.set(r.model, r);
    const all = [...byModel.values()].sort((a, b) => a.model.localeCompare(b.model));
    return q ? all.filter((r) => r.model.toLowerCase().includes(q)) : all;
  }, [modelQuery, catalog]);

  const [suggestionIndex, setSuggestionIndex] = useState(0);
  // The model name committed by the first Enter/click. While the input is
  // exactly `/model <pickedModel>` the picker stays closed so the next Enter
  // sends the command instead of re-picking.
  const [pickedModel, setPickedModel] = useState<string | null>(null);
  useEffect(() => {
    setSuggestionIndex(0);
  }, [modelQuery]);
  useEffect(() => {
    if (pickedModel !== null && input !== `/model ${pickedModel}`) {
      setPickedModel(null);
    }
  }, [input, pickedModel]);

  const showSuggestions =
    modelQuery !== null &&
    pickedModel === null &&
    suggestions.length > 0;

  // Open a persistent SSE stream for the active session.
  // This is the mechanism that delivers binary chunks (audio, images) that
  // arrive after the chat SSE closes at stream_complete.
  useEffect(() => {
    // Clean up previous stream
    cleanupStream.current?.();
    cleanupStream.current = null;
    seenIds.current = new Set(store.messages.map((c) => c.id));
    liveSeenIds.current = new Set();

    const sessionId = store.activeSessionId;
    if (!sessionId) return;

    const close = openSessionStream(
      sessionId,
      (chunk) => {
        const state = useSessionStore.getState();
        // Always feed allChunks (chunk viewer)
        // Avoid double-adding chunks we loaded from history
        if (!seenIds.current.has(chunk.id)) {
          seenIds.current.add(chunk.id);
          state.appendChunk(chunk);
        }
        ingestLive(state, chunk);
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

  // Apply a model choice: publish the model and, when known, the backend that
  // serves it, so a later generation routes to the right provider (ADR-134).
  const applyModel = async (sessionId: string, model: string) => {
    const backend = backendForModel(catalog, model, catalog.default_backend ?? null);
    const annotations: Record<string, unknown> = {
      'config.type': 'runtime',
      'config.llm.model': model,
    };
    if (backend) annotations['config.llm.backend'] = backend.backend;
    try {
      await publishChunk(sessionId, 'null', annotations);
    } catch (err) {
      console.error('[ChatArea] /model error', err);
    }
  };

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

    // /model <name> — select the model and its serving backend for this session
    if (text === '/model' || text.startsWith('/model ')) {
      const name = text.slice('/model'.length).trim();
      if (name) {
        await applyModel(state.activeSessionId, name);
      }
      return;
    }

    state.setStreaming(true);

    await streamChat(
      state.activeSessionId,
      text,
      (chunk) => {
        const s = useSessionStore.getState();
        // Chat SSE delivers text chunks; register them in seenIds so the
        // persistent stream doesn't duplicate them.
        if (!seenIds.current.has(chunk.id)) {
          seenIds.current.add(chunk.id);
          s.appendChunk(chunk);
        }
        ingestLive(s, chunk);
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
    if (showSuggestions) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSuggestionIndex((i) => (i + 1) % suggestions.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSuggestionIndex((i) => (i - 1 + suggestions.length) % suggestions.length);
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
        // First Enter fills the input with the full command; the next Enter
        // (with the picker now closed) sends it.
        e.preventDefault();
        const pick = suggestions[suggestionIndex];
        if (pick) {
          setInput(`/model ${pick.model}`);
          setPickedModel(pick.model);
        }
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setInput('');
        return;
      }
    }
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

  // Filter messages for the chat display. Raw mode shows non-transient chunks
  // (still hiding per-token deltas/tombstones) rather than only chat messages.
  const displayMessages = store.showAllChunks
    ? rawViewChunks(store.allChunks)
    : chatMessagesFrom(store.messages);

  // The session's latest configured model/backend, from runtime config chunks.
  // Used to label the live streaming bubble in raw mode (the per-token deltas
  // carry no model annotation of their own).
  const activeModel = (() => {
    for (let i = store.allChunks.length - 1; i >= 0; i--) {
      const a = store.allChunks[i].annotations;
      const model = a['config.llm.model'];
      if (typeof model === 'string') {
        const backend = a['config.llm.backend'];
        return { model, backend: typeof backend === 'string' ? backend : undefined };
      }
    }
    return null;
  })();

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
          {store.sessions.find((s) => s.session_id === store.activeSessionId)?.parent_id ? (
            <span style={{ marginLeft: 6, fontSize: 12, color: '#8ab4f8' }}>⑂ fork</span>
          ) : null}
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
          <button
            onClick={async () => {
              if (!store.activeSessionId) return;
              setForking(true);
              try {
                await duplicateSession(store.activeSessionId);
              } catch (err) {
                console.error('[ChatArea] fork error', err);
              } finally {
                setForking(false);
              }
            }}
            disabled={forking}
            title="Create a new session copied from this one"
            style={{
              background: 'transparent',
              border: '1px solid #4fc3f7',
              color: forking ? '#666' : '#4fc3f7',
              borderRadius: 4,
              padding: '2px 8px',
              cursor: forking ? 'not-allowed' : 'pointer',
              fontSize: 12,
              fontWeight: 600,
            }}
          >
            {forking ? 'Forking…' : '⑂ Fork'}
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
          <div
            key={chunk.id}
            onClick={() => store.setSelectedChunkId(chunk.id)}
            style={{ cursor: store.chunkViewerOpen ? 'pointer' : undefined }}
          >
            <Message chunk={chunk} raw={store.showAllChunks} />
          </div>
        ))}
        {store.liveStream ? (
          <div
            key={store.liveStream.key}
            style={{ cursor: store.chunkViewerOpen ? 'pointer' : undefined }}
          >
            <Message
              chunk={{
                id: store.liveStream.key,
                content_type: 'text',
                content: store.liveStream.content,
                data: null,
                mime_type: null,
                producer: 'com.nominal.cafe-llm',
                annotations: {
                  'chat.role': 'assistant',
                  'chat.is_streaming': true,
                  ...(activeModel ? { 'chat.model': activeModel.model } : {}),
                  ...(activeModel?.backend ? { 'config.llm.backend': activeModel.backend } : {}),
                },
                timestamp: Date.now(),
              }}
              raw={store.showAllChunks}
            />
          </div>
        ) : null}
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
          position: 'relative',
        }}
      >
        {showSuggestions && (
          <div
            style={{
              position: 'absolute',
              bottom: '100%',
              left: 16,
              right: 16,
              maxHeight: 240,
              overflowY: 'auto',
              background: '#0f3460',
              border: '1px solid #2a2a4a',
              borderRadius: 6,
              marginBottom: 4,
              boxShadow: '0 -4px 16px rgba(0,0,0,0.4)',
              zIndex: 20,
            }}
          >
            {suggestions.map((s, i) => (
              <div
                key={`${s.backend}:${s.model}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setInput(`/model ${s.model}`);
                  setPickedModel(s.model);
                }}
                onMouseEnter={() => setSuggestionIndex(i)}
                style={{
                  display: 'flex',
                  justifyContent: 'space-between',
                  gap: 12,
                  padding: '6px 10px',
                  cursor: 'pointer',
                  fontSize: 13,
                  color: i === suggestionIndex ? '#1a1a2e' : '#ccc',
                  background: i === suggestionIndex ? '#4fc3f7' : 'transparent',
                }}
              >
                <span>{s.model}</span>
                <span style={{ fontSize: 11, opacity: 0.7 }}>{s.backend}</span>
              </div>
            ))}
          </div>
        )}
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
