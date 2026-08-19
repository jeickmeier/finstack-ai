//! Application-supplied authentication and transport restrictions.

use finstack_ai_kernel::Digest;
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

/// Test/reference verifier: loopback method on loopback TCP, bearer secret on Unix/TLS.
///
/// This is a reference implementation. Applications own credential storage,
/// comparison, and rotation. The configured bearer is hashed at construction
/// and never retained as plaintext.
#[derive(Debug, Clone)]
pub struct StaticAuthVerifier {
    bearer_digest: Digest,
    tenant_scope: String,
}

impl StaticAuthVerifier {
    /// Construct a verifier that accepts `bearer` on Unix/TLS.
    #[must_use]
    pub fn new(bearer: impl Into<String>, tenant_scope: impl Into<String>) -> Self {
        Self {
            bearer_digest: bearer_digest(bearer.into().as_bytes()),
            tenant_scope: tenant_scope.into(),
        }
    }
}

impl Default for StaticAuthVerifier {
    fn default() -> Self {
        Self::new("", "")
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
                if !constant_time_eq(
                    self.bearer_digest.as_bytes(),
                    bearer_digest(token.as_bytes()).as_bytes(),
                ) {
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

fn bearer_digest(token: &[u8]) -> Digest {
    Digest::domain_separated("remote-bearer", 1, token).expect("remote-bearer domain is valid")
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut diff = 0_u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::{AuthVerifier, StaticAuthVerifier, TransportKind};
    use crate::ServerError;
    use finstack_ai_protocol::RemoteAuthMethod;

    #[test]
    fn bearer_digest_compare_accepts_exact_secret() {
        let verifier = StaticAuthVerifier::new("secret", "tenant-a");
        let ctx = verifier
            .verify(
                &RemoteAuthMethod::Bearer {
                    token: "secret".into(),
                },
                TransportKind::Unix,
            )
            .expect("match");
        assert_eq!(ctx.principal(), "bearer");
    }

    #[test]
    fn bearer_digest_compare_rejects_prefix_and_wrong_length() {
        let verifier = StaticAuthVerifier::new("secret", "tenant-a");
        for token in ["secre", "secret-extra", "other"] {
            let err = verifier
                .verify(
                    &RemoteAuthMethod::Bearer {
                        token: token.into(),
                    },
                    TransportKind::Tls,
                )
                .expect_err(token);
            assert!(matches!(err, ServerError::AuthenticationFailure), "{token}");
        }
    }
}
