// cafe-types: shared data model for the CAFE architecture
// Chunk, Annotation, ContentType, and wire-format serialization live here.

pub mod annotation;
pub mod chunk;
pub mod envelope;
pub mod error;
pub mod session;

pub use annotation::{keys, roles};
pub use chunk::{Chunk, ContentType};
pub use envelope::{ClientMessage, ServerMessage, SessionConfig};
pub use error::CafeError;
pub use session::{AgentDefinition, SessionInfo};
