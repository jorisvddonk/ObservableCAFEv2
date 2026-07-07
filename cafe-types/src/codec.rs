//! Serialization codec abstraction for the bus wire format.
//!
//! A `BusCodec` combines a serializer (JSON, bincode, etc.) with a framing
//! strategy (newline-delimited, length-prefixed, etc.) into a single unit
//! that can be swapped at compile time.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt;

/// Raw-bytes serde module for `Option<Vec<u8>>` — no base64 encoding.
///
/// Used by binary codecs (bincode, etc.) where compact byte transmission
/// is desired. Selected via the `raw-binary-data` feature flag on Chunk's `data` field.
pub mod raw_bytes_option {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match data {
            None => serializer.serialize_none(),
            Some(bytes) => serializer.serialize_bytes(bytes),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Vec<u8>>::deserialize(deserializer)
    }
}

/// Error type for bus codec operations.
#[derive(Debug)]
pub enum BusCodecError {
    /// I/O error reading from or writing to the socket.
    Io(std::io::Error),
    /// Serialization failed.
    Serialize(String),
    /// Deserialization failed.
    Deserialize(String),
}

impl fmt::Display for BusCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BusCodecError::Io(e) => write!(f, "bus I/O: {}", e),
            BusCodecError::Serialize(msg) => write!(f, "bus serialize: {}", msg),
            BusCodecError::Deserialize(msg) => write!(f, "bus deserialize: {}", msg),
        }
    }
}

impl std::error::Error for BusCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BusCodecError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BusCodecError {
    fn from(e: std::io::Error) -> Self {
        BusCodecError::Io(e)
    }
}

/// A bus wire-format codec: serialization + framing.
///
/// Provides `encode` (serialize + frame) and `decode` (parse + deframe).
/// The framing strategy is up to the implementation:
///
/// - `JsonLineCodec`: JSON objects separated by `\n`
/// - `BincodeLengthPrefixCodec`: bincode with 4-byte LE length prefix
pub trait BusCodec: Send + Sync + 'static {
    /// Human-readable name for diagnostic output.
    const NAME: &'static str;

    /// Encode a message into framed wire bytes.
    fn encode<M: Serialize>(msg: &M) -> Result<Vec<u8>, BusCodecError>;

    /// Try to decode a message from a byte buffer.
    ///
    /// Returns `Some((msg, bytes_consumed))` on success, or `None` if
    /// a complete frame isn't available yet (caller should read more data
    /// and retry).
    fn decode<M: DeserializeOwned>(buf: &[u8]) -> Result<Option<(M, usize)>, BusCodecError>;
}

/// Newline-delimited JSON codec — the default bus wire format.
///
/// Messages are serialized as JSON objects followed by a `\n` newline.
/// The reader scans for `\n` to delimit frames.
pub struct JsonLineCodec;

impl BusCodec for JsonLineCodec {
    const NAME: &'static str = "json";

    fn encode<M: Serialize>(msg: &M) -> Result<Vec<u8>, BusCodecError> {
        let mut json = serde_json::to_string(msg).map_err(|e| BusCodecError::Serialize(e.to_string()))?;
        json.push('\n');
        Ok(json.into_bytes())
    }

    fn decode<M: DeserializeOwned>(buf: &[u8]) -> Result<Option<(M, usize)>, BusCodecError> {
        // Scan for the first newline
        let pos = buf.iter().position(|&b| b == b'\n');
        match pos {
            Some(nl_pos) => {
                let line = &buf[..nl_pos];
                if line.is_empty() {
                    return Err(BusCodecError::Deserialize("empty line".into()));
                }
                let msg: M = serde_json::from_slice(line)
                    .map_err(|e| BusCodecError::Deserialize(format!("JSON: {}", e)))?;
                Ok(Some((msg, nl_pos + 1)))
            }
            None => Ok(None),
        }
    }
}

/// Bincode length-prefixed codec — compact binary wire format.
///
/// Messages are serialized with `bincode` (v2) and prefixed with a 4-byte
/// little-endian length. The reader reads the length first, then the payload.
#[cfg(feature = "bincode-codec")]
pub struct BincodeLengthPrefixCodec;

#[cfg(feature = "bincode-codec")]
impl BusCodec for BincodeLengthPrefixCodec {
    const NAME: &'static str = "bincode";

    fn encode<M: Serialize>(msg: &M) -> Result<Vec<u8>, BusCodecError> {
        let payload = bincode::serde::encode_to_vec(msg, bincode::config::standard())
            .map_err(|e| BusCodecError::Serialize(format!("bincode: {}", e)))?;
        let len = payload.len() as u32;
        let mut framed = Vec::with_capacity(4 + payload.len());
        framed.extend_from_slice(&len.to_le_bytes());
        framed.extend_from_slice(&payload);
        Ok(framed)
    }

    fn decode<M: DeserializeOwned>(buf: &[u8]) -> Result<Option<(M, usize)>, BusCodecError> {
        if buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let total = 4 + len;
        if buf.len() < total {
            return Ok(None);
        }
        let (msg, _): (M, usize) =
            bincode::serde::decode_from_slice(&buf[4..total], bincode::config::standard())
                .map_err(|e| BusCodecError::Deserialize(format!("bincode: {}", e)))?;
        Ok(Some((msg, total)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, ClientMessage, ContentType, ServerMessage};

    fn test_chunk() -> Chunk {
        Chunk::new_text("hello", "test")
    }

    #[test]
    fn json_line_encode_decode_roundtrip() {
        let msg = ClientMessage::Publish {
            session_id: "s-1".into(),
            chunk: test_chunk(),
        };
        let wire = JsonLineCodec::encode(&msg).unwrap();
        let (decoded, consumed) = JsonLineCodec::decode::<ClientMessage>(&wire)
            .unwrap()
            .expect("should decode");
        assert_eq!(consumed, wire.len());
        match decoded {
            ClientMessage::Publish { session_id, .. } => assert_eq!(session_id, "s-1"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn json_line_decode_need_more_data() {
        // Partial JSON — no newline yet
        let partial = b"{\"op\":\"ping\"";
        assert!(JsonLineCodec::decode::<ClientMessage>(partial)
            .unwrap()
            .is_none());
    }

    #[test]
    fn json_line_decode_empty_line_error() {
        let buf = b"\n";
        assert!(JsonLineCodec::decode::<ClientMessage>(buf).is_err());
    }

    #[test]
    fn json_line_server_message_roundtrip() {
        let msg = ServerMessage::Connected {
            connection_id: "c-42".into(),
        };
        let wire = JsonLineCodec::encode(&msg).unwrap();
        let (decoded, _) = JsonLineCodec::decode::<ServerMessage>(&wire)
            .unwrap()
            .expect("should decode");
        match decoded {
            ServerMessage::Connected { connection_id } => assert_eq!(connection_id, "c-42"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn json_line_multiple_messages() {
        let buf = b"{\"op\":\"ping\"}\n{\"op\":\"list_sessions\"}\n";
        let (m1, c1) = JsonLineCodec::decode::<ClientMessage>(buf)
            .unwrap()
            .expect("first");
        assert!(matches!(m1, ClientMessage::Ping));
        let remaining = &buf[c1..];
        let (m2, c2) = JsonLineCodec::decode::<ClientMessage>(remaining)
            .unwrap()
            .expect("second");
        assert!(matches!(m2, ClientMessage::ListSessions));
        assert_eq!(c2, remaining.len());
    }

    #[cfg(feature = "bincode-codec")]
    #[test]
    fn bincode_roundtrip() {
        let msg = ClientMessage::Publish {
            session_id: "s-1".into(),
            chunk: test_chunk(),
        };
        let wire = BincodeLengthPrefixCodec::encode(&msg).unwrap();
        let (decoded, consumed) = BincodeLengthPrefixCodec::decode::<ClientMessage>(&wire)
            .unwrap()
            .expect("should decode");
        assert_eq!(consumed, wire.len());
        match decoded {
            ClientMessage::Publish { session_id, .. } => assert_eq!(session_id, "s-1"),
            _ => panic!("wrong variant"),
        }
    }

    #[cfg(feature = "bincode-codec")]
    #[test]
    fn bincode_decode_need_more_data() {
        // Only 2 bytes — not enough for 4-byte length prefix
        let partial = vec![0u8; 2];
        assert!(BincodeLengthPrefixCodec::decode::<ClientMessage>(&partial)
            .unwrap()
            .is_none());

        // Has length prefix but not enough payload
        let partial = vec![5u8, 0, 0, 0, 1, 2]; // length=5, only 2 bytes payload
        assert!(BincodeLengthPrefixCodec::decode::<ClientMessage>(&partial)
            .unwrap()
            .is_none());
    }
}
