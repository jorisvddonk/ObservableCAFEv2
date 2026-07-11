use std::str::FromStr;
use tracing::info;

use crate::error::SdkError;

use super::transport::BusTransport;

/// Transport over iroh QUIC connections.
#[derive(Clone)]
pub struct IrohTransport {
    endpoint: iroh::Endpoint,
    bus_addr: iroh::EndpointAddr,
    alpn: Vec<u8>,
    description: String,
}

pub struct IrohConfig {
    bus_id: iroh::EndpointId,
    relay_url: Option<iroh::RelayUrl>,
    alpn: Vec<u8>,
    secret_key: Option<iroh::SecretKey>,
    disable_relay: bool,
    bus_addr: Option<iroh::EndpointAddr>,
}

impl IrohConfig {
    pub fn new(bus_id: iroh::EndpointId) -> Self {
        Self {
            bus_id,
            relay_url: None,
            alpn: b"cafe-bus/0".to_vec(),
            secret_key: None,
            disable_relay: false,
            bus_addr: None,
        }
    }

    pub fn with_relay(mut self, relay_url: iroh::RelayUrl) -> Self {
        self.relay_url = Some(relay_url);
        self
    }

    pub fn with_alpn(mut self, alpn: &[u8]) -> Self {
        self.alpn = alpn.to_vec();
        self
    }

    pub fn with_secret_key(mut self, key: iroh::SecretKey) -> Self {
        self.secret_key = Some(key);
        self
    }

    /// Disable relay servers entirely, using only direct connections.
    /// Useful for localhost / same-machine deployments where relay isn't needed.
    pub fn with_direct(mut self) -> Self {
        self.disable_relay = true;
        self
    }

    pub fn from_cli(
        key: Option<&str>,
        relay: Option<&str>,
        alpn: Option<&str>,
    ) -> Option<Self> {
        let key_str = key
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| std::env::var("CAFE_BUS_IROH_KEY").ok())
            .filter(|s| !s.is_empty())?;

        let bus_id = iroh::EndpointId::from_str(&key_str).ok()?;

        let relay_str = relay
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| std::env::var("CAFE_BUS_IROH_RELAY").ok())
            .filter(|s| !s.is_empty());

        let relay_url = relay_str
            .as_deref()
            .and_then(|s| iroh::RelayUrl::from_str(s).ok());

        let alpn_vec = alpn
            .filter(|s| !s.is_empty())
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_else(|| b"cafe-bus/0".to_vec());

        let mut cfg = Self::new(bus_id).with_alpn(&alpn_vec);
        if let Some(url) = relay_url {
            cfg = cfg.with_relay(url);
        }
        Some(cfg)
    }

    /// Create from a serialized `EndpointAddr` (JSON), e.g. from the bus's addr file.
    /// This includes IP addresses for direct connections plus relay for fallback.
    pub fn from_bus_addr_json(json: &str) -> Option<Self> {
        let addr: iroh::EndpointAddr = serde_json::from_str(json).ok()?;
        let alpn = b"cafe-bus/0".to_vec();
        let mut cfg = Self::new(addr.id).with_alpn(&alpn);
        cfg.bus_addr = Some(addr);
        Some(cfg)
    }

    pub async fn bind(self) -> Result<IrohTransport, SdkError> {
        let addr = if let Some(addr) = self.bus_addr {
            addr
        } else {
            let mut addrs: Vec<iroh::TransportAddr> = Vec::new();
            if let Some(ref relay) = self.relay_url {
                addrs.push(iroh::TransportAddr::Relay(relay.clone()));
            }
            iroh::EndpointAddr::from_parts(self.bus_id, addrs)
        };

        let relay_mode = if self.disable_relay {
            iroh::RelayMode::Disabled
        } else {
            iroh::RelayMode::Default
        };

        let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![self.alpn.clone()])
            .relay_mode(relay_mode);

        if let Some(ref sk) = self.secret_key {
            builder = builder.secret_key(sk.clone());
        }
        let endpoint = builder.bind().await
            .map_err(|e| SdkError::BusConnect(e.into()))?;

        endpoint.online().await;

        let description = format!("iroh ({} → {:?})",
            alpn_short(&self.alpn),
            &addr,
        );

        info!("iroh transport ready: {}", description);

        Ok(IrohTransport {
            endpoint,
            bus_addr: addr,
            alpn: self.alpn,
            description,
        })
    }
}

impl IrohTransport {
    pub fn from_endpoint(
        endpoint: iroh::Endpoint,
        bus_addr: iroh::EndpointAddr,
        alpn: &[u8],
    ) -> Self {
        let description = format!("iroh ({} → {:?})",
            alpn_short(alpn),
            &bus_addr,
        );
        Self {
            endpoint,
            bus_addr,
            alpn: alpn.to_vec(),
            description,
        }
    }
}

fn alpn_short(alpn: &[u8]) -> &str {
    std::str::from_utf8(alpn).unwrap_or("?")
}

impl BusTransport for IrohTransport {
    type Reader = iroh::endpoint::RecvStream;
    type Writer = iroh::endpoint::SendStream;

    async fn connect(&self) -> Result<(Self::Writer, Self::Reader), SdkError> {
        tracing::info!("iroh: connect starting for {:?}", self.bus_addr);
        let conn = self
            .endpoint
            .connect(self.bus_addr.clone(), &self.alpn)
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;
        tracing::info!("iroh: connect established, opening stream");

        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;

        tracing::info!("iroh: stream opened");
        Ok((send, recv))
    }

    fn description(&self) -> &str {
        &self.description
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iroh_config_from_cli_no_key() {
        assert!(IrohConfig::from_cli(None, None, None).is_none());
    }

    #[test]
    fn iroh_config_from_cli_empty_key() {
        assert!(IrohConfig::from_cli(Some(""), None, None).is_none());
    }

    #[test]
    fn iroh_config_from_cli_with_builder() {
        let secret = iroh::SecretKey::generate();
        let public: iroh::EndpointId = secret.public();
        let cfg = IrohConfig::new(public).with_alpn(b"test-alpn/1");
        assert_eq!(cfg.alpn, b"test-alpn/1".to_vec());
    }
}
