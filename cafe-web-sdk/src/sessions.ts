import { apiFetch, getBaseUrl, getToken } from './client.js';
import type { SessionInfo, SessionConfig, Chunk, AgentInfo, ModelCatalog } from './types.js';
import { LLM_REGISTRY_SESSION, parseModelCatalog } from './types.js';

export async function listSessions(): Promise<SessionInfo[]> {
  return apiFetch<SessionInfo[]>('/api/sessions');
}

export async function listAgents(): Promise<AgentInfo[]> {
  return apiFetch<AgentInfo[]>('/api/agents');
}

/** Fetch the LLM model catalog (models grouped by backend). */
export async function listModels(): Promise<ModelCatalog> {
  return apiFetch<ModelCatalog>('/api/models');
}

/**
 * Read the model catalog straight from the `_cafe_llm_registry` session
 * history, so the UI reflects exactly what cafe-llm published.
 */
export async function readModelCatalog(): Promise<ModelCatalog> {
  const { chunks } = await getHistory(LLM_REGISTRY_SESSION);
  return parseModelCatalog(chunks);
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

export async function forkSession(
  id: string,
): Promise<{ id: string; parent_id: string }> {
  return apiFetch(`/api/sessions/${id}/fork`, { method: 'POST' });
}

export async function getHistory(
  id: string,
): Promise<{ session_id: string; chunks: Chunk[] }> {
  return apiFetch(`/api/sessions/${id}/history?binaryRefs=1`);
}

export async function setTags(
  sessionId: string,
  tags: string[],
): Promise<void> {
  await apiFetch(`/api/sessions/${sessionId}/tags`, {
    method: 'PATCH',
    body: JSON.stringify({ tags }),
  });
}

export async function publishChunk(
  sessionId: string,
  content_type: string,
  annotations?: Record<string, unknown>,
  content?: string,
): Promise<void> {
  await apiFetch(`/api/sessions/${sessionId}/chunks`, {
    method: 'POST',
    body: JSON.stringify({
      content_type,
      content: content ?? null,
      annotations: annotations ?? null,
    }),
  });
}

/**
 * Delete a chunk. The server publishes a `flow.signal = "delete"` control
 * chunk; the bus drops the target from history and cafe-store removes it, so
 * it disappears live and on reload.
 */
export async function deleteChunk(sessionId: string, chunkId: string): Promise<void> {
  await apiFetch(`/api/sessions/${sessionId}/chunks/${chunkId}`, { method: 'DELETE' });
}

/**
 * Delete several chunks, one control signal each. Never aborts early: every
 * id is attempted and per-id failures are collected, so one bad request cannot
 * leave the rest undeleted. Throws only if every id failed, with the details.
 */
export async function deleteChunks(sessionId: string, chunkIds: string[]): Promise<void> {
  const failures: string[] = [];
  for (const id of chunkIds) {
    try {
      await deleteChunk(sessionId, id);
    } catch (err) {
      failures.push(`${id.slice(0, 8)}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  if (failures.length > 0) {
    throw new Error(
      `${failures.length}/${chunkIds.length} deletes failed:\n${failures.slice(0, 5).join('\n')}`,
    );
  }
}

export function getBinaryUrl(sessionId: string, chunkId: string): string {
  const token = encodeURIComponent(getToken());
  return `${getBaseUrl()}/api/sessions/${sessionId}/chunks/${chunkId}/binary?token=${token}`;
}
