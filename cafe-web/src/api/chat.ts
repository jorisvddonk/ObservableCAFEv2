import { getToken } from './client';
import type { Chunk } from '../types';

/**
 * Send a message and stream the response via fetch + ReadableStream.
 * Calls onChunk for each received chunk, onDone when stream_complete arrives.
 */
export async function streamChat(
  sessionId: string,
  content: string,
  onChunk: (chunk: Chunk) => void,
  onDone: () => void,
  onError: (err: Error) => void,
): Promise<void> {
  try {
    const res = await fetch(`/api/sessions/${sessionId}/chat`, {
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

      // Parse SSE lines
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
