use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

use crate::error::SdkError;

pub trait BusTransport {
    type Reader: AsyncRead + Unpin + Send + 'static;
    type Writer: AsyncWrite + Unpin + Send + 'static;

    async fn connect(&self) -> Result<(Self::Writer, Self::Reader), SdkError>;
    fn description(&self) -> &str;

    /// Log connection path information (relay vs direct). Default no-op.
    fn log_connection_paths(&self) {}

    /// Return connection path information (relay vs direct) as JSON. Default None.
    fn connection_info(&self) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Clone)]
pub struct UnixSocketTransport {
    path: Arc<String>,
}

impl UnixSocketTransport {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: Arc::new(path.into()),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl BusTransport for UnixSocketTransport {
    type Reader = tokio::net::unix::OwnedReadHalf;
    type Writer = tokio::net::unix::OwnedWriteHalf;

    async fn connect(&self) -> Result<(Self::Writer, Self::Reader), SdkError> {
        let stream = UnixStream::connect(self.path.as_str())
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;
        let (reader, writer) = stream.into_split();
        Ok((writer, reader))
    }

    fn description(&self) -> &str {
        self.path.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_transport_description() {
        let t = UnixSocketTransport::new("/tmp/bus.sock");
        assert_eq!(t.description(), "/tmp/bus.sock");
        assert_eq!(t.path(), "/tmp/bus.sock");
    }

    #[test]
    fn unix_transport_clone() {
        let t = UnixSocketTransport::new("/tmp/bus.sock");
        let t2 = t.clone();
        assert_eq!(t.description(), t2.description());
    }
}
