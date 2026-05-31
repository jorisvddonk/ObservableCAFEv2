export type ContentType = 'text' | 'binary' | 'null';

export interface Chunk {
  id: string;
  content_type: ContentType;
  content: string | null;
  data: string | null;       // base64 for binary chunks
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
