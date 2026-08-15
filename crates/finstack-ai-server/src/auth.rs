//! Application-supplied authentication and transport restrictions.

use finstack_ai_protocol::RemoteAuthMethod;

use crate::ServerError;

/// Accepted transport after bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// Unix domain socket.
    Unix,
    /// Loopback plaintext TCP.
    LoopbackPlaintext,
    /// TLS 1.3 TCP.
    Tls,
}

impl TransportKind {
    /// Whether bearer secrets may be presented on this transport.
    #[must_use]
    pub const fn allows_bearer(self) -> bool {
        matches!(self, Self::Unix | Self::Tls)
    }
}

/// Authenticated connection context. Contains no raw tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    tenant_scope: String,
    principal: String,
}

impl AuthContext {
    /// Construct an authenticated context.
    #[must_use]
    pub fn new(tenant_scope: impl Into<String>, principal: impl Into<String>) -> Self {
        Self {
            tenant_scope: tenant_scope.into(),
            principal: principal.into(),
        }
    }

    /// Authenticated tenant scope.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    /// Authenticated principal label.
    #[must_use]
    pub fn principal(&self) -> &str {
        &self.principal
    }
}

/// Application verifier invoked after the pre-auth hello.
pub trait AuthVerifier: Send + Sync + 'static {
    /// Verify `method` for `transport`.
    ///
    /// # Errors
    ///
    /// Must fail closed for bearer-over-plaintext and unknown credentials.
    fn verify(
        &self,
        method: &RemoteAuthMethod,
        transport: TransportKind,
    ) -> Result<AuthContext, ServerError>;
}

/// Test/reference verifier: loopback method on loopback TCP, bearer `secret` on Unix/TLS.
#[derive(Debug, Default)]
pub struct StaticAuthVerifier {
    bearer: String,
    tenant_scope: String,
}

impl StaticAuthVerifier {
    /// Construct a verifier that accepts `bearer` on Unix/TLS.
    #[must_use]
    pub fn new(bearer: impl Into<String>, tenant_scope: impl Into<String>) -> Self {
        Self {
            bearer: bearer.into(),
            tenant_scope: tenant_scope.into(),
        }
    }
}

impl AuthVerifier for StaticAuthVerifier {
    fn verify(
        &self,
        method: &RemoteAuthMethod,
        transport: TransportKind,
    ) -> Result<AuthContext, ServerError> {
        match method {
            RemoteAuthMethod::Bearer { token } => {
                if !transport.allows_bearer() {
                    return Err(ServerError::AuthenticationFailure);
                }
                if token.as_str() != self.bearer {
                    return Err(ServerError::AuthenticationFailure);
                }
                Ok(AuthContext::new(&self.tenant_scope, "bearer"))
            }
            RemoteAuthMethod::Loopback => {
                if transport != TransportKind::LoopbackPlaintext {
                    return Err(ServerError::AuthenticationFailure);
                }
                Ok(AuthContext::new(&self.tenant_scope, "loopback"))
            }
        }
    }
}
