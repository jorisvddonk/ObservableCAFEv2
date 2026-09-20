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
  /** Publish an assistant text chunk from the JS host. */
  publishText(text: string): Promise<{ published: true }>;
  /** Merged runtime config snapshot (see resolve semantics in reference). */
  config(): Promise<Record<string, unknown>>;
  /** Host log. */
  log(msg: string): void;
  sessionId: string;
  agentId: string;
}

declare const cafe: CafeApi;
declare const manifest: JsManifest;
declare function main(cafe: CafeApi): Promise<unknown>;
