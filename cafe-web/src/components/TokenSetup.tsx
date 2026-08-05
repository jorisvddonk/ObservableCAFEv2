import { useState } from 'react';
import { setToken, listSessions, isAuthError } from 'cafe-web-sdk';

interface Props {
  onDone: () => void;
}

export function TokenSetup({ onDone }: Props) {
  const [value, setValue] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const token = value.trim();
    if (!token || busy) return;
    setError(null);
    setBusy(true);
    setToken(token);
    try {
      await listSessions();
      onDone();
    } catch (err) {
      if (isAuthError(err)) {
        setError('That token was rejected by the server. Check the admin token printed to the cafe-server console.');
      } else {
        setError('Could not reach the cafe server. Make sure it is running, then try again.');
      }
    } finally {
      setBusy(false);
    }
  };

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
      <form
        onSubmit={submit}
        style={{
          background: '#16213e',
          border: '1px solid #2a2a4a',
          borderRadius: 12,
          padding: 32,
          width: 360,
          display: 'flex',
          flexDirection: 'column',
          gap: 16,
        }}
      >
        <h1 style={{ color: '#4fc3f7', fontSize: 20, fontWeight: 700 }}>
          CAFE
        </h1>
        <p style={{ color: '#888', fontSize: 13 }}>
          Enter your API token to continue. The admin token is printed to the
          cafe-server console on first startup.
        </p>
        <input
          type="password"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="cafe_adm_…"
          autoFocus
          disabled={busy}
          style={{
            background: '#0f3460',
            border: '1px solid #2a2a4a',
            borderRadius: 6,
            color: '#e0e0e0',
            padding: '8px 12px',
            fontSize: 14,
            outline: 'none',
          }}
        />
        {error && (
          <div
            style={{
              background: '#3a1f2e',
              border: '1px solid #8b3a4a',
              borderRadius: 6,
              color: '#ff8a80',
              padding: '8px 12px',
              fontSize: 13,
            }}
          >
            {error}
          </div>
        )}
        <button
          type="submit"
          disabled={busy || !value.trim()}
          style={{
            background: busy ? '#3a7ea6' : '#4fc3f7',
            color: '#1a1a2e',
            border: 'none',
            borderRadius: 6,
            padding: '10px',
            fontWeight: 700,
            cursor: busy || !value.trim() ? 'not-allowed' : 'pointer',
            fontSize: 14,
            opacity: busy || !value.trim() ? 0.6 : 1,
          }}
        >
          {busy ? 'Checking…' : 'Connect'}
        </button>
      </form>
    </div>
  );
}
