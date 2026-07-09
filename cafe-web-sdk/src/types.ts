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
  tags?: string[];
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
