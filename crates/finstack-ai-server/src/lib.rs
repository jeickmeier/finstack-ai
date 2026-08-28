//! Reference Unix-socket, loopback, and TLS session server.
//!
//! The server implements the bounded session framing contract from
//! `finstack-ai-protocol`. It owns transport policy, authentication, security
//! audit gating, reconnect projection, command receipts, and per-connection
//! credit limits. It does not execute agents or act as production execution
//! authority.
//!
//! # Security model
//!
//! - Loopback plaintext accepts only the loopback authentication method.
//! - Bearer credentials are accepted only over Unix sockets or TLS 1.3.
//! - [`AuthVerifier`] returns a validated [`AuthContext`] containing no raw
//!   credential.
//! - [`Server::bind`] requires a healthy security-audit sink before accepting
//!   external ingress.
//! - Scope mismatches are audited and masked at the connection boundary.
//!
//! [`StaticAuthVerifier`] is a test/reference verifier, not an application
//! credential store. Applications own credential storage, rotation, and
//! authorization policy.
//!
//! # Reference lifecycle
//!
//! [`Server::accept_once`] binds, accepts, and serves one connection. It is
//! intentionally not a production accept loop. The default SDK and runtime
//! remain independent of this crate and of `rustls`.

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
