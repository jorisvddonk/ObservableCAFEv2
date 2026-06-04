import { getToken, getApiBaseUrl } from './client';
import type { Chunk } from '../types';

/**
 * Open a persistent SSE connection to /api/sessions/:id/stream.
 * Requests binary-refs so large audio/image payloads are not inlined.
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
  const token = encodeURIComponent(getToken());
  // binaryRefs=1: binary chunks arrive as lightweight refs; fetched on demand.
  const url = `${base}/api/sessions/${sessionId}/stream?token=${token}&binaryRefs=1`;

  const es = new EventSource(url);

  es.onmessage = (ev) => {
    try {
      const payload = JSON.parse(ev.data);

      if (payload.type === 'history_complete') {
        onHistoryComplete(payload.count ?? 0);
        return;
      }

      if (payload.type === 'error') {
        console.warn('[stream] server error:', payload.message);
        return;
      }

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
