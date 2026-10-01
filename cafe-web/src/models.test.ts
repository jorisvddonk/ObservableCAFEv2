import { describe, it, expect } from 'vitest';
import {
  backendForModel,
  parseModelCatalog,
  CONFIG_MODEL_CATALOG,
  CONFIG_AVAILABLE_MODELS,
} from 'cafe-web-sdk';
import type { Chunk, ModelCatalog } from 'cafe-web-sdk';

function registryChunk(annotations: Record<string, unknown>): Chunk {
  return {
    id: `r${Math.random()}`,
    content_type: 'null',
    content: null,
    data: null,
    mime_type: null,
    producer: 'com.nominal.cafe-llm',
    annotations,
    timestamp: 0,
  };
}

const catalog: ModelCatalog = {
  models: ['gemma3:1b', 'grok-4.6', 'shared'],
  default_backend: 'openai',
  backends: [
    { backend: 'openai', default_model: 'gemma3:1b', models: ['gemma3:1b', 'shared'] },
    { backend: 'opencode-go', default_model: 'deepseek-v4.1-flash', models: ['grok-4.6', 'shared'] },
  ],
};

describe('parseModelCatalog', () => {
  it('reads the structured catalog from the registry session chunks', () => {
    const chunks = [registryChunk({ [CONFIG_MODEL_CATALOG]: JSON.stringify(catalog) })];
    const parsed = parseModelCatalog(chunks);
    expect(parsed.default_backend).toBe('openai');
    expect(parsed.backends).toHaveLength(2);
    expect(parsed.models).toContain('grok-4.6');
  });

  it('uses the newest catalog chunk', () => {
    const older = { ...catalog, default_backend: 'old', backends: [] };
    const chunks = [
      registryChunk({ [CONFIG_MODEL_CATALOG]: JSON.stringify(older) }),
      registryChunk({ [CONFIG_MODEL_CATALOG]: JSON.stringify(catalog) }),
    ];
    expect(parseModelCatalog(chunks).default_backend).toBe('openai');
  });

  it('falls back to the legacy flat model list', () => {
    const chunks = [registryChunk({ [CONFIG_AVAILABLE_MODELS]: JSON.stringify(['a', 'b']) })];
    const parsed = parseModelCatalog(chunks);
    expect(parsed.models).toEqual(['a', 'b']);
    expect(parsed.backends).toEqual([]);
  });

  it('returns an empty catalog when nothing is published', () => {
    expect(parseModelCatalog([]).models).toEqual([]);
  });
});

describe('backendForModel', () => {
  it('returns the backend serving the model', () => {
    expect(backendForModel(catalog, 'grok-4.6')?.backend).toBe('opencode-go');
    expect(backendForModel(catalog, 'gemma3:1b')?.backend).toBe('openai');
  });

  it('prefers the requested backend when several serve the model', () => {
    expect(backendForModel(catalog, 'shared', 'opencode-go')?.backend).toBe('opencode-go');
    expect(backendForModel(catalog, 'shared', 'openai')?.backend).toBe('openai');
  });

  it('returns null for an unknown model', () => {
    expect(backendForModel(catalog, 'nope')).toBeNull();
  });
});
