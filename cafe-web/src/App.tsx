import { useEffect, useState } from 'react';
import { getToken } from './api/client';
import { useSessionStore } from './store/sessions';
import { useSessions } from './hooks/useSessions';
import { Sidebar } from './components/Sidebar';
import { ChatArea } from './components/ChatArea';
import { QuickiesPanel } from './components/QuickiesPanel';
import { TokenSetup } from './components/TokenSetup';

export function App() {
  const [hasToken, setHasToken] = useState(() => !!getToken());
  const { refresh } = useSessions();
  const store = useSessionStore();

  useEffect(() => {
    if (!hasToken) return;
    refresh().then(() => {
      // Auto-select first session or restore from hash
      const hash = window.location.hash.slice(1);
      const sessions = useSessionStore.getState().sessions;
      if (hash && sessions.some((s) => s.session_id === hash)) {
        // useSessions hook handles hash restore
      } else if (sessions.length > 0 && !store.activeSessionId) {
        // Will be handled by useSessions useEffect
      }
    });
  }, [hasToken]);

  if (!hasToken) {
    return <TokenSetup onDone={() => setHasToken(true)} />;
  }

  return (
    <div style={{ display: 'flex', height: '100%' }}>
      <div style={{ display: 'flex', flexDirection: 'column', width: 240, flexShrink: 0 }}>
        <Sidebar />
        <QuickiesPanel />
      </div>
      <ChatArea />
    </div>
  );
}
