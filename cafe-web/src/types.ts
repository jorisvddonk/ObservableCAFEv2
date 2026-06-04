export type ContentType = 'text' | 'binary' | 'null' | 'binary-ref';

/** Metadata object inside a binary-ref chunk's `content` field. */
export interface BinaryRefContent {
  chunk_id: string;
  mime_type: string | null;
  byte_size: number;
}

export interface Chunk {
  id: string;
  content_type: ContentType;
  /** Text content for text chunks; BinaryRefContent (as raw object) for binary-ref chunks. */
  content: string | BinaryRefContent | null;
  data: string | null;       // base64 for full binary chunks (absent for binary-ref)
  mime_type: string | null;
  producer: string;
  annotations: Record<string, unknown>;
  timestamp: number;         // Unix ms
}

export interface SessionInfo {
  session_id: string;
  agent_id: string;
  display_name: string | null;
  is_background: boolean;
  ui_mode: string;
  message_count: number;
  created_at: number;
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
}

// Annotation key constants (mirrors cafe-types)
export const CHAT_ROLE = 'chat.role';
export const CHAT_IS_STREAMING = 'chat.is_streaming';
export const CHAT_STREAM_COMPLETE = 'chat.stream_complete';
export const SECURITY_TRUST_LEVEL = 'security.trust-level';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Narrow a chunk's content to BinaryRefContent if it is a binary-ref. */
export function asBinaryRef(chunk: Chunk): BinaryRefContent | null {
  if (chunk.content_type === 'binary-ref' && chunk.content && typeof chunk.content === 'object') {
    return chunk.content as BinaryRefContent;
  }
  return null;
}

/** True if a chunk carries displayable media (full binary or a binary-ref). */
export function isMediaChunk(chunk: Chunk): boolean {
  return chunk.content_type === 'binary' || chunk.content_type === 'binary-ref';
}

/** Return the mime type for both full binary and binary-ref chunks. */
export function chunkMimeType(chunk: Chunk): string | null {
  if (chunk.content_type === 'binary-ref') {
    return asBinaryRef(chunk)?.mime_type ?? null;
  }
  return chunk.mime_type;
}
