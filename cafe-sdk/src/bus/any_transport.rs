use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::SdkError;

use super::transport::{BusTransport, UnixSocketTransport};

/// Transport that can be either Unix socket or iroh.
///
/// This allows services to decide at runtime which transport to use,
/// so the `BusClient` type stays concrete (`BusClient<AnyTransport>`)
/// throughout the codebase.
#[derive(Clone)]
pub enum AnyTransport {
    Unix(UnixSocketTransport),
    #[cfg(feature = "iroh-client")]
    Iroh(super::iroh_transport::IrohTransport),
}

impl AnyTransport {
    pub fn unix(path: impl Into<String>) -> Self {
        Self::Unix(UnixSocketTransport::new(path))
    }

    #[cfg(feature = "iroh-client")]
    pub fn iroh(transport: super::iroh_transport::IrohTransport) -> Self {
        Self::Iroh(transport)
    }
}

pub enum AnyReader {
    Unix(tokio::net::unix::OwnedReadHalf),
    #[cfg(feature = "iroh-client")]
    Iroh(iroh::endpoint::RecvStream),
}

pub enum AnyWriter {
    Unix(tokio::net::unix::OwnedWriteHalf),
    #[cfg(feature = "iroh-client")]
    Iroh(iroh::endpoint::SendStream),
}

impl AsyncRead for AnyReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(r) => std::pin::Pin::new(r).poll_read(cx, buf),
            #[cfg(feature = "iroh-client")]
            Self::Iroh(r) => std::pin::Pin::new(r).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for AnyWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.get_mut() {
            Self::Unix(w) => std::pin::Pin::new(w).poll_write(cx, buf),
            #[cfg(feature = "iroh-client")]
            Self::Iroh(w) => {
                let pinned = std::pin::Pin::new(w);
                <iroh::endpoint::SendStream as tokio::io::AsyncWrite>::poll_write(pinned, cx, buf)
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Unix(w) => std::pin::Pin::new(w).poll_flush(cx),
            #[cfg(feature = "iroh-client")]
            Self::Iroh(w) => {
                let pinned = std::pin::Pin::new(w);
                <iroh::endpoint::SendStream as tokio::io::AsyncWrite>::poll_flush(pinned, cx)
            }
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Unix(w) => std::pin::Pin::new(w).poll_shutdown(cx),
            #[cfg(feature = "iroh-client")]
            Self::Iroh(w) => {
                let pinned = std::pin::Pin::new(w);
                <iroh::endpoint::SendStream as tokio::io::AsyncWrite>::poll_shutdown(pinned, cx)
            }
        }
    }
}

impl BusTransport for AnyTransport {
    type Reader = AnyReader;
    type Writer = AnyWriter;

    async fn connect(&self) -> Result<(Self::Writer, Self::Reader), SdkError> {
        match self {
            Self::Unix(t) => {
                let (w, r) = t.connect().await?;
                Ok((AnyWriter::Unix(w), AnyReader::Unix(r)))
            }
            #[cfg(feature = "iroh-client")]
            Self::Iroh(t) => {
                let (w, r) = t.connect().await?;
                Ok((AnyWriter::Iroh(w), AnyReader::Iroh(r)))
            }
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::Unix(t) => t.description(),
            #[cfg(feature = "iroh-client")]
            Self::Iroh(t) => t.description(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_transport_unix_description() {
        let t = AnyTransport::unix("/tmp/test.sock");
        assert!(
            t.description().contains("test.sock"),
            "description should contain the socket path: {}",
            t.description()
        );
    }

    #[test]
    fn any_transport_clone() {
        let t = AnyTransport::unix("/tmp/test.sock");
        let t2 = t.clone();
        assert_eq!(t.description(), t2.description());
    }
}
