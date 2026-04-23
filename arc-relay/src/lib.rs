/// arc-relay library root.
///
/// Exposes internal modules so that integration tests in `tests/` can import
/// them without duplicating code.  The binary (`main.rs`) also uses this crate
/// via `use arc_relay::...` — or simply by the modules being in scope because
/// both share the same `src/` tree.
pub mod api;
pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod mirror;
pub mod models;
pub mod relay;
pub mod retention;
pub mod setup;
pub mod signing;
pub mod storage;
pub mod ws;
