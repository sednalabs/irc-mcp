//! Hosted IRC MCP: protocol/authentication via the toolkit, IRC via private Relay.
#![forbid(unsafe_code)]
#[cfg(not(unix))]
compile_error!("The first version requires Unix domain sockets.");
pub mod attention;
pub mod inbox;
pub mod config;
pub mod domain;
pub mod events;
pub mod http;
pub mod mcp;
pub mod receipts;
pub mod relay;
pub mod relay_events;
pub mod service;
