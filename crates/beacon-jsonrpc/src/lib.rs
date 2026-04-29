//! `beacon-jsonrpc` — JSON-RPC 2.0 message types, dispatcher, and (optional) framed transport.
//!
//! # Overview
//!
//! This crate provides the **spine** for Beacon's future JSON-RPC unification (Warp parity M3).
//! Currently it ships:
//!
//! - JSON-RPC 2.0 message types: [`Request`], [`Notification`], [`Response`], [`RpcError`]
//! - A method-dispatch trait: [`RpcHandler`]
//! - A runtime dispatcher: [`RpcDispatcher`]
//! - Optional framed transport (length-prefixed over `AsyncRead`/`AsyncWrite`) behind
//!   the `transport` feature flag.
//!
//! # Feature flags
//!
//! | Flag        | Default | Description                                        |
//! |-------------|---------|-----------------------------------------------------|
//! | `transport` | OFF     | Length-prefixed async reader/writer helpers         |
//!
//! # Migration plan
//!
//! See `MIGRATION.md` in this crate for the remaining hostd↔client, Arc WebSocket, and
//! beacon-remote migration targets.

pub mod dispatcher;
pub mod error;
pub mod message;

#[cfg(feature = "transport")]
pub mod transport;

pub use dispatcher::{BoxFuture, RpcDispatcher, RpcHandler};
pub use error::{RpcError, RpcErrorCode};
pub use message::{Id, Notification, Request, Response};
