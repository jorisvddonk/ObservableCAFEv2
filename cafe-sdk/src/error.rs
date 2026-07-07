use thiserror::Error;

#[derive(Error, Debug)]
pub enum SdkError {
    #[error("bus connection failed: {0}")]
    BusConnect(#[source] anyhow::Error),

    #[error("bus I/O error: {0}")]
    BusIo(#[from] std::io::Error),

    #[error("bus protocol error: {0}")]
    BusProtocol(String),

    #[error("bus codec error: {0}")]
    BusCodec(#[from] cafe_types::BusCodecError),

    #[error("bus returned error: {message} (code: {code:?})")]
    BusError { message: String, code: Option<String> },

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[cfg(feature = "http-client")]
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("timed out")]
    Timeout,

    #[error("bus not ready after {retries} retries")]
    BusNotReady { retries: u32 },
}
