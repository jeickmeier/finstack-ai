//! Reference Unix-socket / loopback session server (session-server framing contract).
//!
//! Default SDK and runtime stay free of this crate and of `rustls`.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

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
