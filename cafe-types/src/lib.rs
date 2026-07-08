// cafe-types: shared data model for the CAFE architecture
// Chunk, Annotation, ContentType, and wire-format serialization live here.

pub mod annotation;
pub mod chunk;
pub mod codec;
pub mod envelope;
pub mod error;
pub mod jsonrpc;
pub mod session;
pub mod step;
pub mod tools;

pub use annotation::{keys, roles};
pub use chunk::{Chunk, ContentType};
pub use codec::{BusCodec, BusCodecError, JsonLineCodec};
pub use envelope::{ClientMessage, EphemeralConfig, ServerMessage, SessionConfig, SubscribeFilter};
pub use error::CafeError;
pub use jsonrpc::{rpc_errors, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use session::{AgentDefinition, SessionInfo};
pub use step::StepDef;
pub use tools::{ToolCall, ToolDefinition, ToolResult};

#[cfg(feature = "bincode-codec")]
pub use codec::BincodeLengthPrefixCodec;
