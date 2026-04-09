//! Foundational domain types for beacon-core.

pub mod block;
pub mod host;
pub mod ids;

pub use block::{Block, BlockFooter, BlockHeader, ShadowCommitId};
pub use host::HostInfo;
pub use ids::{BlockId, SessionId, TabId};
