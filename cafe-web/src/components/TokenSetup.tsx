import { useState } from 'react';
import { setToken } from '../api/client';

interface Props {
  onDone: () => void;
}

export function TokenSetup({ onDone }: Props) {
  const [value, setValue] = useState('');

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!value.trim()) return;
    setToken(value.trim());
    onDone();
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
          ObservableCAFE
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
        <button
          type="submit"
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
          Connect
        </button>
      </form>
    </div>
  );
}
