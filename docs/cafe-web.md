# cafe-web — Build Guide

**Role:** Browser frontend. React SPA that connects to cafe-server's HTTP API.

**Language:** TypeScript + React + Vite  
**Build after:** `cafe-server` is running

---

## What it does

- Session sidebar (list, create, rename, delete, switch)
- Chat interface: send messages, stream responses token by token
- Renders text, images, and audio chunks appropriately
- Trust/untrust UI for web-fetched content
- Quickies panel (one-click agent presets)
- URL hash sync (`#session-id`)
- PWA manifest for installability

---

## Key dependencies

```json
{
  "dependencies": {
    "react": "^18",
    "react-dom": "^18"
  },
  "devDependencies": {
    "typescript": "^5",
    "vite": "^5",
    "@types/react": "^18",
    "@types/react-dom": "^18"
  }
}
```

Add these as needed:
- `zustand` — lightweight state management
- `@tanstack/react-query` — server state / API calls

---

## File structure

```
cafe-web/src/
```

---

## TypeScript types (types.ts)

Mirror the Rust types from cafe-types exactly:

```typescript
export type ContentType = 'text' | 'binary' | 'null';

export interface Chunk {
  id: string;
  content_type: ContentType;
  content: string | null;
  data: string | null;        // base64 for binary chunks
  mime_type: string | null;
  producer: string;
  annotations: Record<string, unknown>;
  timestamp: number;          // Unix ms
}

export interface SessionInfo {
  id: string;
  agent_id: string;
  display_name: string | null;
  is_background: boolean;
  ui_mode: string;
  message_count: number;
  created_at: number;
}

export interface Quickie {
  id: number;
  name: string;
  description: string | null;
  emoji: string | null;
  agent_id: string;
  starter_message: string | null;
  ui_mode: string;
  display_order: number;
}
```

---

## SSE streaming hook (hooks/useSSEStream.ts)

```typescript
export function useSSEStream(
  url: string | null,
  token: string,
  onChunk: (chunk: Chunk) => void,
  onComplete: () => void,
) {
  useEffect(() => {
    if (!url) return;

    // Use fetch + ReadableStream for POST-based SSE (chat endpoint)
    // Use EventSource for GET-based persistent stream
    // ...

    return () => { /* cleanup */ };
  }, [url]);
}
```

Note: `EventSource` only supports GET. For the POST-based chat endpoint, use
`fetch()` with `response.body.getReader()` and parse SSE lines manually.

---

## Rendering chunks (Message.tsx)

```tsx
function Message({ chunk }: { chunk: Chunk }) {
  if (chunk.content_type === 'binary') {
    if (chunk.mime_type?.startsWith('image/')) {
      return <img src={`data:${chunk.mime_type};base64,${chunk.data}`} />;
    }
    if (chunk.mime_type?.startsWith('audio/')) {
      return <audio controls src={`data:${chunk.mime_type};base64,${chunk.data}`} />;
    }
  }

  if (chunk.content_type === 'null') {
    const trustLevel = chunk.annotations['security.trust-level'] as any;
    if (trustLevel?.trusted === false) {
      return <TrustPrompt chunk={chunk} />;
    }
    return null; // don't render other null chunks
  }

  const role = chunk.annotations['chat.role'] as string;
  return (
    <div className={`message message--${role}`}>
      <span className="message__content">{chunk.content}</span>
    </div>
  );
}
```

---

## URL hash sync

On session switch, set `window.location.hash = sessionId`.
On load, read the hash and switch to that session if it exists.

```typescript
useEffect(() => {
  const sessionId = window.location.hash.slice(1);
  if (sessionId) {
    setActiveSession(sessionId);
  }
}, []);

function setActiveSession(id: string) {
  window.location.hash = id;
  store.setActiveSession(id);
}
```

---

## Vite config

```typescript
// vite.config.ts
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': 'http://localhost:3000',  // dev proxy to cafe-server
    },
  },
});
```

---

## PWA manifest (public/manifest.json)

```json
{
  "name": "ObservableCAFE",
  "short_name": "CAFE",
  "start_url": "/",
  "display": "standalone",
  "background_color": "#1a1a2e",
  "theme_color": "#16213e",
  "icons": [
    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png" }
  ]
}
```
