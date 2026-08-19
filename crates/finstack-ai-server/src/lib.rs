//! Reference Unix-socket / loopback session server (PR-058).
//!
//! Default SDK and runtime stay free of this crate and of `rustls`.

#![warn(missing_docs)]

// Request-path order. rustfmt would otherwise alphabetize these.
#[rustfmt::skip]
mod error;
#[rustfmt::skip]
mod listen;
#[rustfmt::skip]
mod auth;
#[rustfmt::skip]
mod frame;
#[rustfmt::skip]
mod session;
#[rustfmt::skip]
mod credit;
#[rustfmt::skip]
mod handshake;
#[rustfmt::skip]
mod connection;
#[rustfmt::skip]
mod server;
#[rustfmt::skip]
mod client;

pub use auth::{AuthContext, AuthVerifier, StaticAuthVerifier, TransportKind};
pub use client::RemoteClient;
pub use credit::CreditLimits;
pub use error::ServerError;
pub use listen::{ListenAddr, tls13_server_config};
pub use server::Server;
pub use session::{ReconnectView, SessionHub, SessionReplica};
