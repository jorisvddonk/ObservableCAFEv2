import { create } from 'zustand';
import type { Chunk, SessionInfo } from '../types';

interface SessionStore {
  sessions: SessionInfo[];
  activeSessionId: string | null;
  messages: Chunk[];
  allChunks: Chunk[];         // raw unfiltered chunks for the chunk viewer
  streaming: boolean;
  streamingText: string;
  chunkViewerOpen: boolean;

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
}

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  messages: [],
  allChunks: [],
  streaming: false,
  streamingText: '',
  chunkViewerOpen: false,

  setSessions: (sessions) => {
    console.log('[store] setSessions count=', sessions.length);
    set({ sessions });
  },
  setActiveSession: (id) => {
    console.log('[store] setActiveSession id=', id);
    set({ activeSessionId: id, messages: [], allChunks: [], streamingText: '' });
  },
  setMessages: (messages) => {
    console.log('[store] setMessages count=', messages.length);
    set({ messages });
  },
  setAllChunks: (chunks) => {
    set({ allChunks: chunks });
  },
  appendChunk: (chunk) => {
    console.log('[store] appendChunk id=', chunk.id, 'role=', chunk.annotations['chat.role']);
    set((s) => ({ messages: [...s.messages, chunk], allChunks: [...s.allChunks, chunk] }));
  },
  appendStreamToken: (text) => {
    console.log('[store] appendStreamToken len=', text.length, 'total=', useSessionStore.getState().streamingText.length + text.length);
    set((s) => ({ streamingText: s.streamingText + text }));
  },
  finaliseStream: (chunk) => {
    console.log('[store] finaliseStream contentLen=', chunk.content?.length ?? 0);
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
}));
