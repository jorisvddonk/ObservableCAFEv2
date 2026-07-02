// cafe-types: shared data model for the CAFE architecture
// Chunk, Annotation, ContentType, and wire-format serialization live here.

pub mod annotation;
pub mod chunk;
pub mod envelope;
pub mod error;
pub mod jsonrpc;
pub mod session;
pub mod tools;

pub use annotation::{keys, roles};
pub use chunk::{Chunk, ContentType};
pub use envelope::{ClientMessage, ServerMessage, SessionConfig};
pub use error::CafeError;
pub use jsonrpc::{rpc_errors, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use session::{AgentDefinition, SessionInfo};
pub use tools::{ToolCall, ToolDefinition, ToolResult};
