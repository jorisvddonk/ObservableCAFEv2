import { getToken, getBaseUrl } from './client.js';
import type { Chunk } from './types.js';

/**
 * Open a persistent SSE connection to /api/sessions/:id/stream.
 * Returns a cleanup function.
 */
export function openSessionStream(
  sessionId: string,
  onChunk: (chunk: Chunk) => void,
  onHistoryComplete: (count: number) => void,
  onError?: (err: Event) => void,
): () => void {
  const token = encodeURIComponent(getToken());
  const url = `${getBaseUrl()}/api/sessions/${sessionId}/stream?token=${token}&binaryRefs=1`;

  const es = new EventSource(url);

  es.onmessage = (ev) => {
    try {
      const payload = JSON.parse(ev.data);

      if (payload.type === 'history_complete') {
        onHistoryComplete(payload.count ?? 0);
        return;
      }

      if (payload.type === 'error') {
        console.warn('[cafe-web-sdk] server error:', payload.message);
        return;
      }

      if (payload.id && payload.content_type) {
        onChunk(payload as Chunk);
      }
    } catch {
      // ignore
    }
  };

  es.onerror = (ev) => {
    onError?.(ev);
  };

  return () => es.close();
}
