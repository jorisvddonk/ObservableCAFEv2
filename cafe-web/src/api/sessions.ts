import { apiFetch, getToken, getApiBaseUrl } from './client';
import type { SessionInfo, SessionConfig, Chunk } from '../types';

export interface AgentInfo {
  id: string;
  description: string;
  background: boolean;
}

export async function listSessions(): Promise<SessionInfo[]> {
  return apiFetch<SessionInfo[]>('/api/sessions');
}

export async function listAgents(): Promise<AgentInfo[]> {
  return apiFetch<AgentInfo[]>('/api/agents');
}

export async function createSession(
  agentId = 'default',
  config?: SessionConfig,
): Promise<{ id: string; agent_id: string }> {
  return apiFetch('/api/sessions', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, config }),
  });
}

export async function deleteSession(id: string): Promise<void> {
  await apiFetch(`/api/sessions/${id}`, { method: 'DELETE' });
}

/**
 * Fetch session history with binary-refs enabled.
 * Binary chunks are returned as lightweight references; the client fetches
 * audio/image data on demand via getBinaryUrl().
 */
export async function getHistory(
  id: string,
): Promise<{ session_id: string; chunks: Chunk[] }> {
  return apiFetch(`/api/sessions/${id}/history?binaryRefs=1`);
}

/**
 * Build the URL for fetching a binary chunk's raw data.
 * Safe to use as <audio src> or <img src> — the server sends immutable cache headers.
 */
export function getBinaryUrl(sessionId: string, chunkId: string): string {
  const base = getApiBaseUrl();
  const token = encodeURIComponent(getToken());
  return `${base}/api/sessions/${sessionId}/chunks/${chunkId}/binary?token=${token}`;
}
