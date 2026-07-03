import { useCallback, useEffect } from 'react';
import { useSessionStore } from '../store/sessions';
import { listSessions, createSession, deleteSession, getHistory } from 'cafe-web-sdk';

export function useSessions() {
  const store = useSessionStore();

  const refresh = useCallback(async () => {
    try {
      const sessions = await listSessions();
      store.setSessions(sessions);
    } catch (err) {
      console.error('Failed to load sessions:', err);
    }
  }, []);

  const switchSession = useCallback(async (id: string) => {
    store.setActiveSession(id);
    const chunkViewerOpen = useSessionStore.getState().chunkViewerOpen;
    window.location.hash = chunkViewerOpen ? `${id}?chunkViewer=1` : id;
    try {
      const { chunks } = await getHistory(id);
      // Raw full history for the chunk viewer
      store.setAllChunks(chunks);
      // Show text chat messages AND binary media (audio/image) from assistant.
      // Includes both full binary chunks and binary-ref placeholders.
      const chatChunks = chunks.filter(
        (c) =>
          (c.content_type === 'text' &&
            (c.annotations['chat.role'] === 'user' ||
              c.annotations['chat.role'] === 'assistant')) ||
          ((c.content_type === 'binary' || c.content_type === 'binary-ref') &&
            c.annotations['chat.role'] === 'assistant'),
      );
      store.setMessages(chatChunks);
    } catch (err) {
      console.error('Failed to load history:', err);
    }
  }, []);

  const newSession = useCallback(async (agentId = 'default') => {
    const { id } = await createSession(agentId);
    await refresh();
    await switchSession(id);
    return id;
  }, [refresh, switchSession]);

  const removeSession = useCallback(
    async (id: string) => {
      await deleteSession(id);
      await refresh();
      const remaining = useSessionStore.getState().sessions;
      if (remaining.length > 0) {
        await switchSession(remaining[0].session_id);
      } else {
        store.setActiveSession(null);
      }
    },
    [refresh, switchSession],
  );

  // Restore session and chunk viewer from URL hash on mount
  useEffect(() => {
    const hash = window.location.hash.slice(1);
    const qIdx = hash.indexOf('?');
    const sessionId = qIdx === -1 ? hash : hash.slice(0, qIdx);
    if (qIdx !== -1) {
      const params = new URLSearchParams(hash.slice(qIdx + 1));
      if (params.get('chunkViewer') === '1') {
        store.setChunkViewerOpen(true);
      }
    }
    if (sessionId) {
      switchSession(sessionId);
    }
  }, []);

  return { refresh, switchSession, newSession, removeSession };
}
