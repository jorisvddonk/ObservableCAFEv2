import { apiFetch } from './client.js';
import type { Quickie } from './types.js';

export async function listQuickies(): Promise<Quickie[]> {
  return apiFetch<Quickie[]>('/api/quickies');
}

export async function deleteQuickie(id: number): Promise<void> {
  await apiFetch(`/api/quickies/${id}`, { method: 'DELETE' });
}
