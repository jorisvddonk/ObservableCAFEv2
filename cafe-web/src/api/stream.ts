import { getToken, getApiBaseUrl } from './client';
import type { Chunk } from '../types';

/**
 * Open a persistent SSE connection to /api/sessions/:id/stream.
 * Delivers history replay followed by all live chunks indefinitely.
 *
 * Returns a cleanup function — call it to close the connection.
 */
export function openSessionStream(
  sessionId: string,
  onChunk: (chunk: Chunk) => void,
  onHistoryComplete: (count: number) => void,
  onError?: (err: Event) => void,
): () => void {
  const base = getApiBaseUrl();
  // EventSource doesn't support custom headers, so pass the token as a query param.
  // cafe-server's auth middleware needs to accept it there — see note below.
  const url = `${base}/api/sessions/${sessionId}/stream?token=${encodeURIComponent(getToken())}`;

  const es = new EventSource(url);

  es.onmessage = (ev) => {
    try {
      const payload = JSON.parse(ev.data);

      // history_complete marker
      if (payload.type === 'history_complete') {
        onHistoryComplete(payload.count ?? 0);
        return;
      }

      // error marker
      if (payload.type === 'error') {
        console.warn('[stream] server error:', payload.message);
        return;
      }

      // Regular chunk
      if (payload.id && payload.content_type) {
        onChunk(payload as Chunk);
      }
    } catch {
      // ignore parse errors
    }
  };

  es.onerror = (ev) => {
    onError?.(ev);
  };

  return () => es.close();
}
