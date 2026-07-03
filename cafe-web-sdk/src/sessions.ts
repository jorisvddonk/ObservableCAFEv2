import { apiFetch, getBaseUrl, getToken } from './client.js';
import type { SessionInfo, SessionConfig, Chunk, AgentInfo } from './types.js';

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

export async function getHistory(
  id: string,
): Promise<{ session_id: string; chunks: Chunk[] }> {
  return apiFetch(`/api/sessions/${id}/history?binaryRefs=1`);
}

export function getBinaryUrl(sessionId: string, chunkId: string): string {
  const token = encodeURIComponent(getToken());
  return `${getBaseUrl()}/api/sessions/${sessionId}/chunks/${chunkId}/binary?token=${token}`;
}
