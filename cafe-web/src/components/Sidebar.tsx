import { useEffect, useState } from 'react';
import { useSessionStore } from '../store/sessions';
import { useSessions } from '../hooks/useSessions';
import { listAgents } from '../api/sessions';
import type { SessionInfo } from '../types';
import type { AgentInfo } from '../api/sessions';

export function Sidebar() {
  const { sessions, activeSessionId } = useSessionStore();
  const { switchSession, newSession } = useSessions();

  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [creating, setCreating] = useState(false);

  // Load agent list once on mount
  useEffect(() => {
    listAgents()
      .then((list) => setAgents(list.filter((a) => !a.background)))
      .catch(() => {/* ignore — fallback to default */});
  }, []);

  const handleNew = () => {
    // If we only have one (or zero) foreground agents, skip the picker
    if (agents.length <= 1) {
      const agentId = agents[0]?.id ?? 'default';
      setCreating(true);
      newSession(agentId).finally(() => setCreating(false));
    } else {
      setPickerOpen((v) => !v);
    }
  };

  const handlePick = (agentId: string) => {
    setPickerOpen(false);
    setCreating(true);
    newSession(agentId).finally(() => setCreating(false));
  };

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

      {/* New session button */}
      <button
        onClick={handleNew}
        disabled={creating}
        style={{
          margin: '0 12px 4px',
          padding: '6px 10px',
          background: '#0f3460',
          color: creating ? '#666' : '#e0e0e0',
          border: '1px solid #2a2a4a',
          borderRadius: 6,
          cursor: creating ? 'not-allowed' : 'pointer',
          fontSize: 13,
          textAlign: 'left',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <span>{creating ? 'Creating…' : '+ New session'}</span>
        {agents.length > 1 && (
          <span style={{ fontSize: 10, color: '#4fc3f7', marginLeft: 4 }}>
            {pickerOpen ? '▲' : '▼'}
          </span>
        )}
      </button>

      {/* Agent picker dropdown */}
      {pickerOpen && agents.length > 1 && (
        <div
          style={{
            margin: '0 12px 6px',
            border: '1px solid #2a2a4a',
            borderRadius: 6,
            overflow: 'hidden',
            background: '#0d1b33',
          }}
        >
          {agents.map((a) => (
            <button
              key={a.id}
              onClick={() => handlePick(a.id)}
              style={{
                display: 'block',
                width: '100%',
                padding: '7px 10px',
                background: 'transparent',
                color: '#ccc',
                border: 'none',
                borderBottom: '1px solid #1a2a40',
                cursor: 'pointer',
                textAlign: 'left',
                fontSize: 12,
              }}
            >
              <div style={{ fontWeight: 600, color: '#4fc3f7' }}>{a.id}</div>
              {a.description && (
                <div style={{ fontSize: 11, color: '#666', marginTop: 1 }}>
                  {a.description}
                </div>
              )}
            </button>
          ))}
        </div>
      )}

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
