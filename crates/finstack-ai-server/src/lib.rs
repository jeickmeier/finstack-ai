//! Reference Unix-socket / loopback session server (PR-058).
//!
//! Default SDK and runtime stay free of this crate and of `rustls`.

#![warn(missing_docs)]

mod auth;
mod client;
mod connection;
mod error;
mod io;
mod listen;
mod replica;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use finstack_ai_runtime::{SecurityAuditGate, SecurityAuditSink};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

pub use auth::{AuthContext, AuthVerifier, StaticAuthVerifier, TransportKind};
pub use client::{ReconnectView, RemoteClient};
pub use connection::ConnectionLimits;
pub use error::ServerError;
pub use listen::{ListenAddr, SERVER_LISTEN_INVALID, tls13_server_config};
pub use replica::{CreditLimits, CreditWindow, ReconnectPlan, SessionHub, SessionReplica};

use crate::connection::serve_connection;
use crate::listen::install_ring;
use crate::replica::CreditWindow as Window;

/// Ready reference server. Construction enables the audit gate.
pub struct Server {
    listen: ListenAddr,
    auth: Arc<dyn AuthVerifier>,
    audit: Arc<SecurityAuditGate>,
    hub: Arc<SessionHub>,
    limits: ConnectionLimits,
    credit: CreditLimits,
    ids: AtomicU64,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

impl Server {
    /// Enable the audit gate and validate the listen policy.
    ///
    /// # Errors
    ///
    /// Fails closed for an invalid listen address, missing/unhealthy sink, or
    /// an invalid default version offer.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// use finstack_ai_runtime::{
    ///     SecurityAuditHealth, SecurityAuditReceipt, SecurityAuditSink,
    /// };
    /// use finstack_ai_server::{ListenAddr, Server, StaticAuthVerifier};
    ///
    /// struct Ready;
    /// impl SecurityAuditSink for Ready {
    ///     fn record(
    ///         &self,
    ///         event: finstack_ai_runtime::SecurityAuditEvent,
    ///     ) -> finstack_ai_runtime::PortFuture<
    ///         Result<SecurityAuditReceipt, finstack_ai_runtime::SecurityAuditError>,
    ///     > {
    ///         let event_id = std::sync::Arc::<str>::from(event.event_id());
    ///         Box::pin(async move {
    ///             Ok(SecurityAuditReceipt {
    ///                 event_id,
    ///                 recorded_at: event.timestamp(),
    ///             })
    ///         })
    ///     }
    ///     fn health(
    ///         &self,
    ///     ) -> finstack_ai_runtime::PortFuture<
    ///         Result<SecurityAuditHealth, finstack_ai_runtime::SecurityAuditError>,
    ///     > {
    ///         Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let server = Server::bind(
    ///     ListenAddr::loopback(0),
    ///     Arc::new(StaticAuthVerifier::new("secret", "tenant-a")),
    ///     Some(Arc::new(Ready)),
    ///     Duration::from_millis(100),
    /// )
    /// .await
    /// .expect("ready");
    /// assert!(server.hub().with("missing", |_| Ok(())).is_err());
    /// # });
    /// ```
    pub async fn bind(
        listen: ListenAddr,
        auth: Arc<dyn AuthVerifier>,
        sink: Option<Arc<dyn SecurityAuditSink>>,
        audit_deadline: Duration,
    ) -> Result<Self, ServerError> {
        install_ring();
        listen.validate()?;
        let audit = SecurityAuditGate::enable(sink, audit_deadline)
            .await
            .map_err(|_| ServerError::AuditNotReady)?;
        Ok(Self {
            listen,
            auth,
            audit,
            hub: Arc::new(SessionHub::default()),
            limits: ConnectionLimits::v1()?,
            credit: CreditLimits::default(),
            ids: AtomicU64::new(1),
        })
    }

    /// Shared session hub.
    #[must_use]
    pub fn hub(&self) -> &Arc<SessionHub> {
        &self.hub
    }

    /// Override credit limits before accept.
    pub fn set_credit_limits(&mut self, credit: CreditLimits) {
        self.credit = credit;
    }

    /// Bind and accept one TCP or Unix connection, then serve it.
    ///
    /// # Errors
    ///
    /// Returns listen, I/O, or protocol failures.
    pub async fn accept_once(&self) -> Result<(), ServerError> {
        match &self.listen {
            ListenAddr::Loopback { .. } | ListenAddr::Tcp { tls: None, .. } => {
                let addr = self.listen.tcp_addr().ok_or(ServerError::ListenInvalid)?;
                let bind = addr.to_string();
                let listener = TcpListener::bind(&bind).await?;
                let (stream, _) = listener.accept().await?;
                self.serve_tcp(stream, TransportKind::LoopbackPlaintext)
                    .await
            }
            ListenAddr::Tcp {
                addr,
                tls: Some(tls),
            } => {
                let listener = TcpListener::bind(*addr).await?;
                let (stream, _) = listener.accept().await?;
                let connector = tokio_rustls::TlsAcceptor::from(Arc::clone(tls));
                let stream = connector.accept(stream).await?;
                self.serve_tls(stream).await
            }
            #[cfg(unix)]
            ListenAddr::Unix { path } => {
                let _ = std::fs::remove_file(path);
                let listener = UnixListener::bind(path)?;
                let (stream, _) = listener.accept().await?;
                self.serve_unix(stream).await
            }
            #[cfg(not(unix))]
            ListenAddr::Unix { .. } => Err(ServerError::ListenInvalid),
        }
    }

    /// Serve an already-accepted loopback TCP stream.
    ///
    /// # Errors
    ///
    /// Returns protocol or I/O failures.
    pub async fn serve_tcp(
        &self,
        stream: tokio::net::TcpStream,
        transport: TransportKind,
    ) -> Result<(), ServerError> {
        self.serve(stream, transport).await
    }

    /// Serve an already-accepted TLS stream.
    ///
    /// # Errors
    ///
    /// Returns protocol or I/O failures.
    pub async fn serve_tls(
        &self,
        stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    ) -> Result<(), ServerError> {
        self.serve(stream, TransportKind::Tls).await
    }

    /// Serve an already-accepted Unix stream.
    ///
    /// # Errors
    ///
    /// Returns protocol or I/O failures.
    #[cfg(unix)]
    pub async fn serve_unix(&self, stream: tokio::net::UnixStream) -> Result<(), ServerError> {
        self.serve(stream, TransportKind::Unix).await
    }

    pub(crate) async fn serve<S>(
        &self,
        stream: S,
        transport: TransportKind,
    ) -> Result<(), ServerError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let connection_id = self.ids.fetch_add(1, Ordering::Relaxed);
        serve_connection(
            stream,
            transport,
            Arc::clone(&self.auth),
            Arc::clone(&self.audit),
            Arc::clone(&self.hub),
            self.limits.clone(),
            Window::new(self.credit),
            connection_id,
        )
        .await
    }
}

#[cfg(test)]
mod tests;
