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

  useEffect(() => {
    console.log('[App] mount origin=', window.location.origin);
    if (!hasToken) return;
    refresh().then(() => {
      console.log('[App] refresh complete, sessions=', useSessionStore.getState().sessions.length);
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
