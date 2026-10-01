export type ContentType = 'text' | 'binary' | 'binary-ref' | 'null';

export interface BinaryRefContent {
  chunk_id: string;
  mime_type: string | null;
  byte_size?: number;
}

export interface Chunk {
  id: string;
  content_type: ContentType;
  content: string | BinaryRefContent | null;
  data: string | null;
  mime_type: string | null;
  producer: string;
  annotations: Record<string, unknown>;
  timestamp: number;
}

export interface SessionInfo {
  session_id: string;
  agent_id: string;
  display_name: string | null;
  tags: string[];
  is_background: boolean;
  ui_mode: string;
  message_count: number;
  created_at: number;
  parent_id?: string | null;
}

export interface Quickie {
  id: number;
  name: string;
  description: string | null;
  emoji: string | null;
  agent_id: string;
  starter_message: string | null;
  ui_mode: string;
  display_order: number;
}

export interface SessionConfig {
  backend?: string;
  model?: string;
  system_prompt?: string;
  temperature?: number;
  max_tokens?: number;
  compaction_mode?: string;
  max_history_messages?: number;
  max_history_chars?: number;
  tags?: string[];
}

/** The models a single LLM backend serves, and its default. */
export interface BackendModels {
  backend: string;
  default_model?: string | null;
  models: string[];
}

/** The LLM model catalog exposed by cafe-server at `GET /api/models`. */
export interface ModelCatalog {
  /** Flat list of every model across all backends. */
  models: string[];
  /** Backend used when a session sets no `config.llm.backend`. */
  default_backend?: string | null;
  backends: BackendModels[];
}

/** The session cafe-llm publishes its model catalog to. */
export const LLM_REGISTRY_SESSION = '_cafe_llm_registry';
/** Annotation on a registry chunk holding a serialized {@link ModelCatalog}. */
export const CONFIG_MODEL_CATALOG = 'config.model_catalog';
/** Legacy flat annotation on a registry chunk holding model names. */
export const CONFIG_AVAILABLE_MODELS = 'config.available_models';

/**
 * Parse the model catalog out of the `_cafe_llm_registry` session's chunks.
 *
 * Reads the structured `config.model_catalog` annotation (which carries backend
 * + default model per model) from the newest chunk that has it, falling back to
 * the legacy flat `config.available_models` list.
 */
export function parseModelCatalog(chunks: Chunk[]): ModelCatalog {
  for (let i = chunks.length - 1; i >= 0; i--) {
    const raw = chunks[i].annotations[CONFIG_MODEL_CATALOG];
    if (typeof raw === 'string') {
      try {
        const parsed = JSON.parse(raw) as ModelCatalog;
        if (parsed && Array.isArray(parsed.backends)) {
          return {
            models: parsed.models ?? parsed.backends.flatMap((b) => b.models),
            default_backend: parsed.default_backend ?? null,
            backends: parsed.backends,
          };
        }
      } catch {
        // fall through to the legacy key
      }
    }
  }
  for (let i = chunks.length - 1; i >= 0; i--) {
    const raw = chunks[i].annotations[CONFIG_AVAILABLE_MODELS];
    if (typeof raw === 'string') {
      try {
        const models = JSON.parse(raw) as string[];
        if (Array.isArray(models)) {
          return { models, default_backend: null, backends: [] };
        }
      } catch {
        // ignore
      }
    }
  }
  return { models: [], default_backend: null, backends: [] };
}

/** The backend serving `model`, preferring `preferred` when several match. */
export function backendForModel(
  catalog: ModelCatalog,
  model: string,
  preferred?: string | null,
): BackendModels | null {
  const candidates = catalog.backends.filter((b) => b.models.includes(model));
  if (preferred) {
    const hit = candidates.find((b) => b.backend === preferred);
    if (hit) return hit;
  }
  return candidates[0] ?? null;
}

export interface AgentInfo {
  id: string;
  description: string;
  background: boolean;
}

// Annotation key constants (mirrors cafe-types)
export const CHAT_ROLE = 'chat.role';
export const CHAT_MODEL = 'chat.model';
export const CHAT_IS_STREAMING = 'chat.is_streaming';
export const CHAT_STREAM_COMPLETE = 'chat.stream_complete';
export const CHAT_FINISH_REASON = 'chat.finish_reason';
export const ERROR_MESSAGE = 'cafe.error.message';
export const FLOW_SIGNAL = 'cafe.flow.signal';
export const FLOW_TOMBSTONE = 'cafe.flow.tombstone';
export const MUTATES_TARGET_ID = 'cafe.mutates.target_id';
export const SECURITY_TRUST_LEVEL = 'security.trust-level';
export const BINARY_READ_URL = 'cafe.binary.read_url';
export const BINARY_READ_TOKEN = 'cafe.binary.read_token';

/** Check if a chunk is a binary asset (full binary or binary-ref). */
export function isMediaChunk(chunk: Chunk): boolean {
  return chunk.content_type === 'binary' || chunk.content_type === 'binary-ref';
}

/** Narrow to BinaryRefContent if the chunk is a binary-ref. */
export function asBinaryRef(chunk: Chunk): BinaryRefContent | null {
  if (chunk.content_type === 'binary-ref' && chunk.content && typeof chunk.content === 'object') {
    return chunk.content as BinaryRefContent;
  }
  return null;
}

/** Return the MIME type for both full binary and binary-ref chunks. */
export function chunkMimeType(chunk: Chunk): string | null {
  if (chunk.content_type === 'binary-ref') {
    return asBinaryRef(chunk)?.mime_type ?? null;
  }
  return chunk.mime_type;
}
