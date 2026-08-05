import { useEffect, useState } from 'react';
import { getToken, clearToken, isAuthError } from 'cafe-web-sdk';
import { useSessionStore } from './store/sessions';
import { useSessions } from './hooks/useSessions';
import { Sidebar } from './components/Sidebar';
import { ChatArea } from './components/ChatArea';
import { QuickiesPanel } from './components/QuickiesPanel';
import { TokenSetup } from './components/TokenSetup';
import { ChunkViewer } from './components/ChunkViewer';

function useIsMobile() {
  const [m, setM] = useState(() => window.innerWidth < 640);
  useEffect(() => {
    const h = () => setM(window.innerWidth < 640);
    window.addEventListener('resize', h);
    return () => window.removeEventListener('resize', h);
  }, []);
  return m;
}

export function App() {
  const [hasToken, setHasToken] = useState(() => !!getToken());
  const [authError, setAuthError] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const { refresh } = useSessions();
  const { chunkViewerOpen, toggleChunkViewer, activeSessionId, showAllChunks } = useSessionStore();
  const isMobile = useIsMobile();

  useEffect(() => {
    console.log('[App] mount origin=', window.location.origin);
    if (!hasToken) return;
    refresh().then(() => {
      console.log('[App] refresh complete, sessions=', useSessionStore.getState().sessions.length);
    }).catch((err) => {
      if (isAuthError(err)) {
        console.error('[App] auth failed:', err);
        setAuthError(true);
      }
    });
  }, [hasToken]);

  const handleLogout = () => {
    clearToken();
    useSessionStore.getState().reset();
    setAuthError(false);
    setHasToken(false);
  };

  // Sync chunk viewer and raw state to URL hash
  useEffect(() => {
    if (!activeSessionId) return;
    const hash = window.location.hash.slice(1);
    const qIdx = hash.indexOf('?');
    const sessionId = qIdx === -1 ? hash : hash.slice(0, qIdx);
    if (!sessionId) return;
    const params = new URLSearchParams();
    if (chunkViewerOpen) params.set('chunkViewer', '1');
    if (showAllChunks) params.set('raw', '1');
    const qs = params.toString();
    window.location.hash = qs ? `${sessionId}?${qs}` : sessionId;
  }, [chunkViewerOpen, showAllChunks, activeSessionId]);

  if (!hasToken) {
    return <TokenSetup onDone={() => setHasToken(true)} />;
  }

  if (authError) {
    return (
      <div
        style={{
          height: '100%',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          background: '#1a1a2e',
        }}
      >
        <div
          style={{
            background: '#16213e',
            border: '1px solid #2a2a4a',
            borderRadius: 12,
            padding: 32,
            width: 380,
            display: 'flex',
            flexDirection: 'column',
            gap: 16,
            textAlign: 'center',
          }}
        >
          <h1 style={{ color: '#ff6b6b', fontSize: 18, fontWeight: 700 }}>
            Authentication failed
          </h1>
          <p style={{ color: '#888', fontSize: 13 }}>
            Your token was rejected by the cafe server. It may be wrong or
            expired.
          </p>
          <button
            onClick={handleLogout}
            style={{
              background: '#4fc3f7',
              color: '#1a1a2e',
              border: 'none',
              borderRadius: 6,
              padding: '10px',
              fontWeight: 700,
              cursor: 'pointer',
              fontSize: 14,
            }}
          >
            Log out
          </button>
        </div>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      {/* Top bar */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: isMobile ? 'space-between' : 'flex-end',
          padding: '4px 12px',
          background: '#16213e',
          borderBottom: '1px solid #2a2a4a',
          flexShrink: 0,
        }}
      >
        {isMobile && (
          <button
            onClick={() => setSidebarOpen((v) => !v)}
            style={{
              background: 'transparent',
              color: '#aaa',
              border: '1px solid #2a2a4a',
              borderRadius: 4,
              padding: '3px 8px',
              fontSize: 16,
              cursor: 'pointer',
              lineHeight: 1,
            }}
          >
            {sidebarOpen ? '✕' : '☰'}
          </button>
        )}
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
          paddingBottom: chunkViewerOpen ? '40vh' : 0,
        }}
      >
        {/* Desktop sidebar — always visible */}
        {!isMobile && (
          <div style={{ display: 'flex', flexDirection: 'column', width: 240, flexShrink: 0, minHeight: 0 }}>
            <Sidebar />
            <QuickiesPanel />
          </div>
        )}

        {/* Mobile sidebar — overlay when open */}
        {isMobile && sidebarOpen && (
          <>
            <div
              onClick={() => setSidebarOpen(false)}
              style={{
                position: 'fixed',
                inset: 0,
                background: 'rgba(0,0,0,0.5)',
                zIndex: 10,
              }}
            />
            <div
              style={{
                position: 'fixed',
                top: 0,
                left: 0,
                bottom: 0,
                width: 260,
                zIndex: 11,
                display: 'flex',
                flexDirection: 'column',
                background: '#16213e',
                borderRight: '1px solid #2a2a4a',
                boxShadow: '4px 0 20px rgba(0,0,0,0.3)',
                animation: 'slideIn 0.2s ease',
                overflowY: 'auto',
              }}
            >
              <Sidebar onSelectSession={() => setSidebarOpen(false)} />
              <QuickiesPanel />
            </div>
          </>
        )}

        <ChatArea />
      </div>

      <ChunkViewer zIndex={isMobile && sidebarOpen ? 5 : 1000} />
    </div>
  );
}
