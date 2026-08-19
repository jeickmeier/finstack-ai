//! Reference Unix-socket / loopback session server (PR-058).
//!
//! Default SDK and runtime stay free of this crate and of `rustls`.

#![warn(missing_docs)]

mod auth;
mod client;
mod connection;
mod credit;
mod error;
mod frame;
mod handshake;
mod listen;
mod server;
mod session;

pub use auth::{AuthContext, AuthVerifier, StaticAuthVerifier, TransportKind};
pub use client::RemoteClient;
pub use credit::CreditLimits;
pub use error::ServerError;
pub use listen::{ListenAddr, tls13_server_config};
pub use server::Server;
pub use session::{ReconnectView, SessionHub, SessionReplica};
