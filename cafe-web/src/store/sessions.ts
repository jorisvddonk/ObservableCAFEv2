import { create } from 'zustand';
import type { Chunk, SessionInfo } from '../types';

interface SessionStore {
  sessions: SessionInfo[];
  activeSessionId: string | null;
  messages: Chunk[];
  streaming: boolean;
  streamingText: string;

  setSessions: (sessions: SessionInfo[]) => void;
  setActiveSession: (id: string | null) => void;
  setMessages: (chunks: Chunk[]) => void;
  appendChunk: (chunk: Chunk) => void;
  appendStreamToken: (text: string) => void;
  finaliseStream: (chunk: Chunk) => void;
  setStreaming: (v: boolean) => void;
  clearStreamingText: () => void;
}

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: [],
  activeSessionId: null,
  messages: [],
  streaming: false,
  streamingText: '',

  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (id) => set({ activeSessionId: id, messages: [], streamingText: '' }),
  setMessages: (messages) => set({ messages }),
  appendChunk: (chunk) => set((s) => ({ messages: [...s.messages, chunk] })),
  appendStreamToken: (text) =>
    set((s) => ({ streamingText: s.streamingText + text })),
  finaliseStream: (chunk) =>
    set((s) => ({
      messages: [...s.messages, chunk],
      streamingText: '',
      streaming: false,
    })),
  setStreaming: (v) => set({ streaming: v }),
  clearStreamingText: () => set({ streamingText: '' }),
}));
