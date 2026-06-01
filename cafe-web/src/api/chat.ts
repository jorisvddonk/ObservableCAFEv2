import { getToken } from './client';
import type { Chunk } from '../types';

function resolveApiBase(): string {
  if (typeof window === 'undefined') return '';

  const explicit = (window as any).__CAFE_API_URL__;
  if (typeof explicit === 'string' && explicit.length > 0) {
    return explicit;
  }

  const origin = window.location.origin;
  const m = origin.match(/^(https?:\/\/[^:]+):(\d+)$/);
  if (m) {
    const port = parseInt(m[2], 10);
    if (port === 8081) {
      return `${m[1]}:4000`;
    }
    return origin;
  }

  return `${origin}:4000`;
}

export async function streamChat(
  sessionId: string,
  content: string,
  onChunk: (chunk: Chunk) => void,
  onDone: () => void,
  onError: (err: Error) => void,
): Promise<void> {
  try {
    const base = resolveApiBase();
    const url = `${base}/api/sessions/${sessionId}/chat`;
    console.log('[chat.ts] origin=', window.location.origin, 'base=', base, 'url=', url);
    const res = await fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${getToken()}`,
      },
      body: JSON.stringify({ content }),
    });

    if (!res.ok || !res.body) {
      throw new Error(`${res.status} ${res.statusText}`);
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });

      let newlineIdx: number;
      while ((newlineIdx = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, newlineIdx).trim();
        buffer = buffer.slice(newlineIdx + 1);

        if (!line.startsWith('data: ')) continue;
        const jsonStr = line.slice('data: '.length);
        try {
          const chunk = JSON.parse(jsonStr) as Chunk;
          onChunk(chunk);
          if (chunk.annotations['chat.stream_complete'] === true) {
            onDone();
            return;
          }
        } catch {
          // skip malformed lines
        }
      }
    }
    onDone();
  } catch (err) {
    onError(err instanceof Error ? err : new Error(String(err)));
  }
}
