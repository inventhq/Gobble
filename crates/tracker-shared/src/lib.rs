//! Shared types and utilities used by tracker-core and standalone consumer binaries.

pub mod event;
pub mod health;

mod resolve;
pub use resolve::resolve_server_addr;
