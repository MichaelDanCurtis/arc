//! Wire protocol types shared between beacon clients and Arc.
//!
//! Encoded with MessagePack (`rmp-serde`) over wss. Protocol version
//! is negotiated in the `HostHello`/`AttachSession` handshake.

pub const PROTOCOL_VERSION: u16 = 1;

pub use beacon_model::{Block, BlockFooter, BlockHeader, BlockId, SessionId, TabId};

pub mod codec;
pub mod envelope;
pub use codec::{decode, encode, CodecError};
pub use envelope::{ArcToClient, ClientToArc};

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn has_protocol_version() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
