use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("msgpack encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("msgpack decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
}

/// Encode any `Serialize` type to MessagePack bytes.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    Ok(rmp_serde::to_vec_named(value)?)
}

/// Decode MessagePack bytes into any `DeserializeOwned` type.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    Ok(rmp_serde::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{ArcToClient, ClientToArc};
    use beacon_model::{BlockId, SessionId};

    #[test]
    fn msgpack_round_trip_client_to_arc() {
        let original = ClientToArc::BlockOutputChunk {
            session_id: SessionId::new(),
            block_id: BlockId::new(),
            seq: 42,
            bytes: b"hello world\n".to_vec(),
        };
        let encoded = encode(&original).unwrap();
        assert!(!encoded.is_empty());
        let decoded: ClientToArc = decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn decode_reports_error_on_garbage() {
        let r: Result<ClientToArc, _> = decode(&[0xff, 0xff, 0xff]);
        assert!(r.is_err());
    }

    #[test]
    fn msgpack_round_trip_arc_to_client_ping() {
        let original = ArcToClient::Ping;
        let encoded = encode(&original).unwrap();
        let decoded: ArcToClient = decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }
}
