import { create } from 'zustand';
import type { Chunk, SessionInfo } from 'cafe-web-sdk';

function applyMutations(chunks: Chunk[]): Chunk[] {
  const mutations = chunks.filter(
    (c) => c.annotations['cafe.mutates.target_id'] as string
      || c.annotations['mutates.target_id'] as string,
  );
  if (mutations.length === 0) return chunks;
  return chunks.map((c) => {
    const mutation = mutations.find(
      (m) => (m.annotations['cafe.mutates.target_id'] as string
        || m.annotations['mutates.target_id'] as string) === c.id,
    );
    if (!mutation) return c;
    const merged = { ...c, annotations: { ...c.annotations } };
    for (const k in mutation.annotations) {
      if (k !== 'cafe.mutates.target_id' && k !== 'mutates.target_id') {
        merged.annotations[k] = mutation.annotations[k];
      }
    }
    return merged;
  });
}

interface SessionStore {
  sessions: SessionInfo[];
  activeSessionId: string | null;
  messages: Chunk[];
  allChunks: Chunk[];         // raw unfiltered chunks for the chunk viewer
  streaming: boolean;
  streamingText: string;
  chunkViewerOpen: boolean;
  showAllChunks: boolean;

  setSessions: (sessions: SessionInfo[]) => void;
  setActiveSession: (id: string | null) => void;
  setMessages: (chunks: Chunk[]) => void;
  setAllChunks: (chunks: Chunk[]) => void;
  appendChunk: (chunk: Chunk) => void;
  appendStreamToken: (text: string) => void;
  finaliseStream: (chunk: Chunk) => void;
  setStreaming: (v: boolean) => void;
  clearStreamingText: () => void;
  toggleChunkViewer: () => void;
  setChunkViewerOpen: (v: boolean) => void;
  toggleShowAllChunks: () => void;
  setShowAllChunks: (v: boolean) => void;
}

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  messages: [],
  allChunks: [],
  streaming: false,
  streamingText: '',
  chunkViewerOpen: false,
  showAllChunks: false,

  setSessions: (sessions) => {
    console.log('[store] setSessions count=', sessions.length);
    set({ sessions });
  },
  setActiveSession: (id) => {
    console.log('[store] setActiveSession id=', id);
    set({ activeSessionId: id, messages: [], allChunks: [], streamingText: '' });
  },
  setMessages: (messages) => {
    const merged = applyMutations(messages);
    console.log('[store] setMessages count=', merged.length);
    set({ messages: merged });
  },
  setAllChunks: (chunks) => {
    const merged = applyMutations(chunks);
    set({ allChunks: merged });
  },
  appendChunk: (chunk) => {
    // Handle mutation: merge annotations into target chunk in both allChunks and messages
    const mutationKey = chunk.annotations['cafe.mutates.target_id'] as string
      || chunk.annotations['mutates.target_id'] as string;
    if (mutationKey) {
      const targetId = mutationKey;
      set((s) => ({
        messages: s.messages.map((c) => {
          if (c.id === targetId) {
            const merged = { ...c, annotations: { ...c.annotations } };
            for (const k in chunk.annotations) {
              if (k !== 'cafe.mutates.target_id' && k !== 'mutates.target_id') {
                merged.annotations[k] = chunk.annotations[k];
              }
            }
            return merged;
          }
          return c;
        }),
        allChunks: s.allChunks.map((c) => {
          if (c.id === targetId) {
            const merged = { ...c, annotations: { ...c.annotations } };
            for (const k in chunk.annotations) {
              if (k !== 'cafe.mutates.target_id' && k !== 'mutates.target_id') {
                merged.annotations[k] = chunk.annotations[k];
              }
            }
            return merged;
          }
          return c;
        }),
      }));
      return;
    }
    console.log('[store] appendChunk id=', chunk.id, 'role=', chunk.annotations['chat.role']);
    set((s) => {
      if (s.allChunks.some((c) => c.id === chunk.id)) return s;
      return { messages: [...s.messages, chunk], allChunks: [...s.allChunks, chunk] };
    });
  },
  appendStreamToken: (text) => {
    console.log('[store] appendStreamToken len=', text.length, 'total=', useSessionStore.getState().streamingText.length + text.length);
    set((s) => ({ streamingText: s.streamingText + text }));
  },
  finaliseStream: (chunk) => {
    // Handle mutation: merge annotations into target chunk in both allChunks and messages
    const mutationKey = chunk.annotations['cafe.mutates.target_id'] as string
      || chunk.annotations['mutates.target_id'] as string;
    if (mutationKey) {
      const targetId = mutationKey;
      set((s) => ({
        messages: s.messages.map((c) => {
          if (c.id === targetId) {
            const merged = { ...c, annotations: { ...c.annotations } };
            for (const k in chunk.annotations) {
              if (k !== 'cafe.mutates.target_id' && k !== 'mutates.target_id') {
                merged.annotations[k] = chunk.annotations[k];
              }
            }
            return merged;
          }
          return c;
        }),
        allChunks: s.allChunks.map((c) => {
          if (c.id === targetId) {
            const merged = { ...c, annotations: { ...c.annotations } };
            for (const k in chunk.annotations) {
              if (k !== 'cafe.mutates.target_id' && k !== 'mutates.target_id') {
                merged.annotations[k] = chunk.annotations[k];
              }
            }
            return merged;
          }
          return c;
        }),
        streamingText: '',
        streaming: false,
      }));
      return;
    }
    console.log('[store] finaliseStream contentLen=', typeof chunk.content === 'string' ? chunk.content.length : 0);
    set((s) => ({
      messages: [...s.messages, chunk],
      allChunks: [...s.allChunks, chunk],
      streamingText: '',
      streaming: false,
    }));
  },
  setStreaming: (v) => {
    console.log('[store] setStreaming=', v);
    set({ streaming: v });
  },
  clearStreamingText: () => {
    console.log('[store] clearStreamingText');
    set({ streamingText: '' });
  },
  toggleChunkViewer: () => {
    set((s) => ({ chunkViewerOpen: !s.chunkViewerOpen }));
  },
  setChunkViewerOpen: (v) => {
    set({ chunkViewerOpen: v });
  },
  toggleShowAllChunks: () => {
    set((s) => ({ showAllChunks: !s.showAllChunks }));
  },
  setShowAllChunks: (v: boolean) => {
    set({ showAllChunks: v });
  },
}));
