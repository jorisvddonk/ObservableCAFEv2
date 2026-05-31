interface Props {
  text: string;
}

export function StreamingIndicator({ text }: Props) {
  if (!text) return null;
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-start',
        marginBottom: 12,
      }}
    >
      <span style={{ fontSize: 11, color: '#888', marginBottom: 2 }}>assistant</span>
      <div
        style={{
          background: '#0f3460',
          borderRadius: 8,
          padding: '8px 12px',
          maxWidth: '80%',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
          lineHeight: 1.5,
        }}
      >
        {text}
        <span
          style={{
            display: 'inline-block',
            width: 8,
            height: '1em',
            background: '#4fc3f7',
            marginLeft: 2,
            verticalAlign: 'text-bottom',
            animation: 'blink 1s step-end infinite',
          }}
        />
      </div>
      <style>{`@keyframes blink { 50% { opacity: 0; } }`}</style>
    </div>
  );
}
