import { apiFetch } from './client';
import type { SessionInfo, SessionConfig } from '../types';

export async function listSessions(): Promise<SessionInfo[]> {
  return apiFetch<SessionInfo[]>('/api/sessions');
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

export async function getHistory(id: string): Promise<{ session_id: string; chunks: import('../types').Chunk[] }> {
  return apiFetch(`/api/sessions/${id}/history`);
}
