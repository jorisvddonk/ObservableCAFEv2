use thiserror::Error;

#[derive(Debug, Error)]
pub enum CafeError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session already exists: {0}")]
    SessionExists(String),

    #[error("agent not found: {0}")]
    AgentNotFound(String),

    #[error("invalid message: {0}")]
    InvalidMessage(String),

    #[error("payload too large")]
    PayloadTooLarge,

    #[error("{0}")]
    Other(String),
}

impl CafeError {
    /// Returns the bus error code string for this error.
    pub fn code(&self) -> &'static str {
        match self {
            CafeError::SessionNotFound(_) => "SESSION_NOT_FOUND",
            CafeError::SessionExists(_) => "SESSION_EXISTS",
            CafeError::AgentNotFound(_) => "AGENT_NOT_FOUND",
            CafeError::InvalidMessage(_) => "INVALID_MESSAGE",
            CafeError::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            _ => "INTERNAL_ERROR",
        }
    }
}
