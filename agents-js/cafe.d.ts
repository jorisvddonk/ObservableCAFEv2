/**
 * Type declarations for the `cafe` bridge object available to JS agents.
 *
 * This file is documentation for editors only — it is never loaded by
 * `cafe-agent-js`. The runtime contract is docs/reference/js-agents.md;
 * if the two disagree, the reference wins.
 */

export type JsEventType = "user_message" | "llm_complete" | "tick";

export interface JsEvent {
  type: JsEventType;
  text: string;
  /** Id of the chunk that triggered the event. */
  id: string;
  /** Content type of the trigger chunk: "text" | "binary" | "binary_ref" | "null". */
  content_type: string;
  /** `chat.role` of the trigger chunk, if any. */
  role?: string;
  /** Annotations of the trigger chunk. */
  annotations: Record<string, unknown>;
}

/** One chunk summary from `cafe.history()`. */
export interface HistoryChunk {
  id: string;
  content_type: string;
  content: string | null;
  role: string | null;
  producer: string;
  mime_type: string | null;
  timestamp: number;
  transient: boolean;
  /** Whether inline binary data is present (the bytes themselves are omitted). */
  has_data: boolean;
  annotations: Record<string, unknown>;
}

/** A parsed `<|tool_call|>` marker. */
export interface ToolCallSpec {
  name: string;
  parameters?: Record<string, unknown>;
  provider?: string;
}

export interface JsManifest {
  /** Unique id; session ID for background agents. Required. */
  name: string;
  description?: string;
  background?: boolean;
  allows_reload?: boolean;
  persists_state?: boolean;
  /** "stateless" (default) or "stateful". */
  mode?: "stateless" | "stateful";
  /** 7-field cron with seconds (see ticker.js for an example). */
  schedule?: string;
  rpc_timeout_secs?: number;
  /** Annotations for the config-seeding null chunk. */
  initial_config?: Record<string, unknown>;
}

export interface FetchInit {
  method?: string;
  headers?: Record<string, string> | [string, string][];
  body?: string;
}

/** Headers-like view of a response (get/has/forEach/entries/keys/values). */
export interface FetchHeaders {
  get(name: string): string | null;
  has(name: string): boolean;
  forEach(cb: (value: string, name: string) => void): void;
  entries(): [string, string][];
  keys(): string[];
  values(): string[];
}

export interface FetchResponse {
  ok: boolean;
  status: number;
  statusText: string;
  url: string;
  headers: FetchHeaders;
  /** Standard web/security annotations for this fetch (untrusted content). */
  annotations: Record<string, unknown>;
  text(): Promise<string>;
  json(): Promise<any>;
  /**
   * Publish the fetched body as a text chunk carrying its web/security
   * annotations (untrusted by default).
   */
  publish(options?: PublishInit): Promise<PublishResult>;
}

/** Spec for `cafe.publish` (and the `options` of `cafe.publishText`). */
export interface PublishInit {
  /** Chunk content type. Default "text". */
  type?: "text" | "null" | "binary";
  /** Text content (for `type: "text"`). */
  content?: string;
  /** Base64-encoded bytes (for `type: "binary"`). */
  data?: string;
  /** MIME type (for `type: "binary"`; default application/octet-stream). */
  mime_type?: string;
  /** Sets `chat.role`; text chunks default to "assistant". */
  role?: string;
  /** Arbitrary annotations, applied verbatim (e.g. "config.*", "cafe.flow.signal"). */
  annotations?: Record<string, unknown>;
  /** Publish as transient (not persisted). */
  transient?: boolean;
  /** Transient retention window in seconds (implies transient). */
  retain_secs?: number;
}

export interface PublishResult {
  published: true;
  id: string;
}

export interface CafeApi {
  /** Async generator of session events; ends when the stream closes. */
  events(): AsyncGenerator<JsEvent>;
  /** `{evaluator}.invoke` RPC. Rejects with Error on RPC error/timeout. */
  invoke(evaluator: string, params?: unknown): Promise<any>;
  /** Raw RPC for any bus method. Same resolve/reject contract. */
  rpc(method: string, params?: unknown): Promise<any>;
  /**
   * Bus-RPC tool call: dispatches `name`, publishes `cafe.tool.result` +
   * readable text for follow-up LLM turns. Resolves with the tool output.
   */
  tool(name: string, params?: unknown): Promise<any>;
  /**
   * Publish a chunk (text or null) with arbitrary annotations.
   * Returns the published chunk id.
   */
  publish(spec: PublishInit): Promise<PublishResult>;
  /**
   * Publish an assistant text chunk (sugar over `publish`). Pass `options`
   * to attach annotations, e.g. `publishText("hi", { annotations: { k: 1 } })`.
   */
  publishText(text: string, options?: PublishInit): Promise<PublishResult>;
  /** Publish a null chunk carrying only annotations (config, signals, metadata). */
  annotate(annotations: Record<string, unknown>): Promise<PublishResult>;
  /**
   * Outbound HTTP, shaped like the browser Fetch API. Resolves for HTTP error
   * statuses (ok:false); rejects with TypeError on transport errors. Also
   * available as the global `fetch`.
   */
  fetch(url: string, options?: FetchInit): Promise<FetchResponse>;
  /** Merged runtime config snapshot (see resolve semantics in reference). */
  config(): Promise<Record<string, unknown>>;
  /** Session history: chunk summaries, oldest first. */
  history(): Promise<HistoryChunk[]>;
  /** Parse `<|tool_call|>` markers from LLM text (host-consistent parsing). */
  findToolCalls(text: string): ToolCallSpec[];
  /** Host log. */
  log(msg: string): void;
  sessionId: string;
  agentId: string;
}

declare const cafe: CafeApi;
declare const manifest: JsManifest;
declare function main(cafe: CafeApi): Promise<unknown>;

/** `cafe.fetch` is also exposed as a global, Fetch-API-shaped function. */
declare function fetch(url: string, options?: FetchInit): Promise<FetchResponse>;
