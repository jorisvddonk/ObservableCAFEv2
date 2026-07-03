# ADR-006: JSON-RPC over bus annotations

**Status**: Accepted

**Context**: Services need to invoke remote procedures (TTS synthesis, ComfyUI image generation, LLM completion). Options: direct HTTP between services (creates coupling), a dedicated RPC channel on the bus (new message types), or embedding RPC in existing chunk infrastructure.

**Decision**: RPC is embedded in chunk annotations using standard JSON-RPC 2.0:
- Request: annotation `jsonrpc.request` → `JsonRpcRequest { id, method, params }`
- Response: annotation `jsonrpc.response` → `JsonRpcResponse { id, result, error }`

The pipeline dispatches an RPC by publishing a transient chunk with `jsonrpc.request`. The target service receives it via its subscription, processes it, and publishes a transient response chunk with `jsonrpc.response`. The pipeline picks up the response from its subscription stream.

**Consequences**:
- No new message types — RPC is just another annotation
- Any service can expose RPC methods without bus changes
- Request/response correlation via call ID (standard JSON-RPC)
- RPC chunks are transient — not persisted (ADR-005)
- Late subscribers miss RPC messages (led to ADR-104 retention buffer)
- Pipeline must subscribe + wait for response (potential timeout)
