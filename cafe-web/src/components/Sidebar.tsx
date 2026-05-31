import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import type { SessionInfo } from '../types';

export function Sidebar() {
  const { sessions, activeSessionId } = useSessionStore();
  const { switchSession, newSession } = useSessions();

  return (
    <aside
      style={{
        width: 240,
        background: '#16213e',
        borderRight: '1px solid #2a2a4a',
        display: 'flex',
        flexDirection: 'column',
        flexShrink: 0,
      }}
    >
      <div
        style={{
          padding: '16px 12px 8px',
          fontWeight: 700,
          fontSize: 15,
          color: '#4fc3f7',
          letterSpacing: 0.5,
        }}
      >
        ObservableCAFE
      </div>

      <button
        onClick={() => newSession()}
        style={{
          margin: '0 12px 8px',
          padding: '6px 10px',
          background: '#0f3460',
          color: '#e0e0e0',
          border: '1px solid #2a2a4a',
          borderRadius: 6,
          cursor: 'pointer',
          fontSize: 13,
          textAlign: 'left',
        }}
      >
        + New session
      </button>

      <div style={{ overflowY: 'auto', flex: 1 }}>
        {sessions
          .filter((s) => !s.is_background)
          .map((s) => (
            <SessionItem
              key={s.session_id}
              session={s}
              active={s.session_id === activeSessionId}
              onSelect={() => switchSession(s.session_id)}
            />
          ))}
      </div>
    </aside>
  );
}

function SessionItem({
  session,
  active,
  onSelect,
}: {
  session: SessionInfo;
  active: boolean;
  onSelect: () => void;
}) {
  const name =
    session.display_name ?? session.session_id.slice(0, 8) + '…';

  return (
    <button
      onClick={onSelect}
      style={{
        display: 'block',
        width: '100%',
        padding: '8px 12px',
        background: active ? '#0f3460' : 'transparent',
        color: active ? '#4fc3f7' : '#ccc',
        border: 'none',
        borderLeft: active ? '3px solid #4fc3f7' : '3px solid transparent',
        cursor: 'pointer',
        textAlign: 'left',
        fontSize: 13,
      }}
    >
      <div style={{ fontWeight: active ? 600 : 400 }}>{name}</div>
      <div style={{ fontSize: 11, color: '#666', marginTop: 2 }}>
        {session.agent_id} · {session.message_count} msgs
      </div>
    </button>
  );
}
