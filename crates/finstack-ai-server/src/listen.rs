//! Listen-address policy (TDD §28.2).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::ServerError;

/// Reference-server bind target.
#[derive(Clone)]
pub enum ListenAddr {
    /// Filesystem Unix socket. Bearer secrets may be used here.
    Unix {
        /// Socket path.
        path: PathBuf,
    },
    /// TCP bind. Non-loopback requires [`ListenAddr::tls`]. Loopback
    /// (`127.0.0.1`) is plaintext only.
    Tcp {
        /// Bind address.
        addr: SocketAddr,
        /// TLS 1.3 server configuration. `None` is plaintext and is valid
        /// only when `addr` is loopback; non-loopback plaintext fails closed.
        tls: Option<Arc<ServerConfig>>,
    },
}

impl ListenAddr {
    /// Loopback TCP on `127.0.0.1`. Bearer secrets are rejected on this
    /// transport. `0` asks the OS for an ephemeral port.
    #[must_use]
    pub const fn loopback(port: u16) -> Self {
        Self::Tcp {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            tls: None,
        }
    }

    /// Unix-socket listen target.
    #[must_use]
    pub fn unix(path: impl Into<PathBuf>) -> Self {
        Self::Unix { path: path.into() }
    }

    /// Plaintext TCP. Non-loopback addresses fail [`ListenAddr::validate`].
    #[must_use]
    pub const fn plaintext_tcp(addr: SocketAddr) -> Self {
        Self::Tcp { addr, tls: None }
    }

    /// Non-loopback TCP with a TLS 1.3-only rustls configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::ListenInvalid`] when `addr` is loopback.
    pub fn tls(addr: SocketAddr, tls: Arc<ServerConfig>) -> Result<Self, ServerError> {
        if addr.ip().is_loopback() {
            return Err(ServerError::ListenInvalid);
        }
        Ok(Self::Tcp {
            addr,
            tls: Some(tls),
        })
    }

    /// Fail closed for non-loopback plaintext.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::ListenInvalid`] when the address violates policy.
    pub fn validate(&self) -> Result<(), ServerError> {
        match self {
            Self::Unix { .. } => Ok(()),
            Self::Tcp { addr, tls } => {
                if addr.ip().is_loopback() {
                    if tls.is_some() {
                        return Err(ServerError::ListenInvalid);
                    }
                    return Ok(());
                }
                if tls.is_none() {
                    return Err(ServerError::ListenInvalid);
                }
                Ok(())
            }
        }
    }

    /// Socket address used for TCP binds.
    #[must_use]
    pub fn tcp_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::Tcp { addr, .. } => Some(*addr),
            Self::Unix { .. } => None,
        }
    }

    /// TLS configuration when the bind is non-loopback TCP.
    #[must_use]
    pub fn tls_config(&self) -> Option<&Arc<ServerConfig>> {
        match self {
            Self::Tcp { tls, .. } => tls.as_ref(),
            Self::Unix { .. } => None,
        }
    }
}

/// Build a TLS 1.3-only rustls server configuration.
///
/// # Errors
///
/// Returns [`ServerError::ListenInvalid`] when rustls rejects the certificate
/// material.
pub fn tls13_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, ServerError> {
    install_ring();
    let config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|_| ServerError::ListenInvalid)?;
    Ok(Arc::new(config))
}

pub(crate) fn install_ring() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::ListenAddr;
    use crate::ServerError;

    #[test]
    fn non_loopback_plaintext_is_rejected() {
        let addr = ListenAddr::plaintext_tcp("0.0.0.0:9".parse().expect("addr"));
        assert!(matches!(
            addr.validate(),
            Err(ServerError::ListenInvalid)
        ));
    }

    #[test]
    fn loopback_plaintext_is_accepted() {
        ListenAddr::loopback(0).validate().expect("loopback");
    }

    #[test]
    fn loopback_constructor_is_plaintext_localhost() {
        let addr = ListenAddr::loopback(9);
        assert_eq!(
            addr.tcp_addr(),
            Some("127.0.0.1:9".parse().expect("addr"))
        );
        assert!(addr.tls_config().is_none());
    }
}
