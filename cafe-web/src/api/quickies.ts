import { apiFetch } from './client';
import type { Quickie } from '../types';

export async function listQuickies(): Promise<Quickie[]> {
  return apiFetch<Quickie[]>('/api/quickies');
}

export async function deleteQuickie(id: number): Promise<void> {
  await apiFetch(`/api/quickies/${id}`, { method: 'DELETE' });
}
