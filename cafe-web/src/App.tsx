import { useEffect, useState } from 'react';
import { getToken } from './api/client';
import { useSessionStore } from './store/sessions';
import { useSessions } from './hooks/useSessions';
import { Sidebar } from './components/Sidebar';
import { ChatArea } from './components/ChatArea';
import { QuickiesPanel } from './components/QuickiesPanel';
import { TokenSetup } from './components/TokenSetup';
import { ChunkViewer } from './components/ChunkViewer';

export function App() {
  const [hasToken, setHasToken] = useState(() => !!getToken());
  const { refresh } = useSessions();
  const { chunkViewerOpen, toggleChunkViewer, activeSessionId } = useSessionStore();

  useEffect(() => {
    console.log('[App] mount origin=', window.location.origin);
    if (!hasToken) return;
    refresh().then(() => {
      console.log('[App] refresh complete, sessions=', useSessionStore.getState().sessions.length);
    });
  }, [hasToken]);

  // Sync chunk viewer open state to URL hash
  useEffect(() => {
    if (!activeSessionId) return;
    const hash = window.location.hash.slice(1);
    const qIdx = hash.indexOf('?');
    const sessionId = qIdx === -1 ? hash : hash.slice(0, qIdx);
    if (!sessionId) return;
    window.location.hash = chunkViewerOpen ? `${sessionId}?chunkViewer=1` : sessionId;
  }, [chunkViewerOpen, activeSessionId]);

  if (!hasToken) {
    return <TokenSetup onDone={() => setHasToken(true)} />;
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      {/* Top bar */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'flex-end',
          padding: '4px 12px',
          background: '#16213e',
          borderBottom: '1px solid #2a2a4a',
          flexShrink: 0,
        }}
      >
        <button
          onClick={toggleChunkViewer}
          disabled={!activeSessionId}
          title="Toggle chunk viewer"
          style={{
            background: chunkViewerOpen ? '#4fc3f7' : '#0f3460',
            color: chunkViewerOpen ? '#1a1a2e' : '#aaa',
            border: '1px solid #2a2a4a',
            borderRadius: 4,
            padding: '3px 10px',
            fontSize: 12,
            cursor: activeSessionId ? 'pointer' : 'not-allowed',
            opacity: activeSessionId ? 1 : 0.4,
            fontFamily: 'monospace',
            fontWeight: 600,
          }}
        >
          {'{ } Chunks'}
        </button>
      </div>

      {/* Main content */}
      <div
        style={{
          display: 'flex',
          flex: 1,
          minHeight: 0,
          // Leave room at the bottom when chunk viewer is open
          paddingBottom: chunkViewerOpen ? '40vh' : 0,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'column', width: 240, flexShrink: 0 }}>
          <Sidebar />
          <QuickiesPanel />
        </div>
        <ChatArea />
      </div>

      <ChunkViewer />
    </div>
  );
}
