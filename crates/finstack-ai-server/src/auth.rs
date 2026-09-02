//! Application-supplied authentication and transport restrictions.

use finstack_ai_kernel::{Digest, label_is_valid};
use finstack_ai_protocol::{PROTOCOL_VERSION_V1, RemoteAuthMethod};

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
    protocol_version: u16,
}

impl AuthContext {
    /// Construct a validated authenticated context.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::AuthenticationConfigurationInvalid`] when the
    /// tenant scope or principal is not a bounded semantic label.
    pub fn try_new(
        tenant_scope: impl Into<String>,
        principal: impl Into<String>,
    ) -> Result<Self, ServerError> {
        let tenant_scope = tenant_scope.into();
        let principal = principal.into();
        if !label_is_valid(&tenant_scope) || !label_is_valid(&principal) {
            return Err(ServerError::AuthenticationConfigurationInvalid);
        }
        Ok(Self {
            tenant_scope,
            principal,
            protocol_version: PROTOCOL_VERSION_V1,
        })
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

    /// Negotiated envelope version for this connection.
    #[must_use]
    pub const fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    pub(crate) const fn with_protocol_version(mut self, protocol_version: u16) -> Self {
        self.protocol_version = protocol_version;
        self
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
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::AuthenticationConfigurationInvalid`] when the
    /// bearer is empty or `tenant_scope` is not a bounded semantic label.
    pub fn try_new(
        bearer: impl Into<String>,
        tenant_scope: impl Into<String>,
    ) -> Result<Self, ServerError> {
        let bearer = bearer.into();
        let tenant_scope = tenant_scope.into();
        if bearer.is_empty() || !label_is_valid(&tenant_scope) {
            return Err(ServerError::AuthenticationConfigurationInvalid);
        }
        Ok(Self {
            bearer_digest: bearer_digest(bearer.as_bytes()),
            tenant_scope,
        })
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
                AuthContext::try_new(&self.tenant_scope, "bearer")
                    .map_err(|_| ServerError::AuthenticationFailure)
            }
            RemoteAuthMethod::Loopback => {
                if transport != TransportKind::LoopbackPlaintext {
                    return Err(ServerError::AuthenticationFailure);
                }
                AuthContext::try_new(&self.tenant_scope, "loopback")
                    .map_err(|_| ServerError::AuthenticationFailure)
            }
        }
    }
}

fn bearer_digest(token: &[u8]) -> Digest {
    finstack_ai_kernel::fixed_domain_digest!("remote-bearer", 1, token)
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
    use super::{AuthContext, AuthVerifier, StaticAuthVerifier, TransportKind};
    use crate::ServerError;
    use finstack_ai_protocol::RemoteAuthMethod;

    #[test]
    fn auth_context_rejects_empty_or_malformed_identity() {
        for (tenant, principal) in [
            ("", "principal"),
            ("tenant-a", ""),
            ("tenant\0a", "principal"),
            ("tenant-a", "principal\0label"),
        ] {
            let error = AuthContext::try_new(tenant, principal).expect_err("invalid identity");
            assert!(matches!(
                error,
                ServerError::AuthenticationConfigurationInvalid
            ));
            assert_eq!(error.code(), "authentication_configuration_invalid");
        }
    }

    #[test]
    fn static_verifier_rejects_empty_credentials_or_scope() {
        for (bearer, tenant) in [("", "tenant-a"), ("secret", ""), ("secret", "tenant\0a")] {
            let error = StaticAuthVerifier::try_new(bearer, tenant).expect_err("invalid verifier");
            assert!(matches!(
                error,
                ServerError::AuthenticationConfigurationInvalid
            ));
            assert_eq!(error.code(), "authentication_configuration_invalid");
        }
    }

    #[test]
    fn bearer_digest_compare_accepts_exact_secret() {
        let verifier =
            StaticAuthVerifier::try_new("secret", "tenant-a").expect("valid static verifier");
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
        let verifier =
            StaticAuthVerifier::try_new("secret", "tenant-a").expect("valid static verifier");
        for token in ["", "secre", "secret-extra", "other"] {
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

    #[test]
    fn verifier_enforces_transport_specific_methods() {
        let verifier =
            StaticAuthVerifier::try_new("secret", "tenant-a").expect("valid static verifier");
        assert!(matches!(
            verifier.verify(&RemoteAuthMethod::Loopback, TransportKind::Unix),
            Err(ServerError::AuthenticationFailure)
        ));
        assert!(matches!(
            verifier.verify(
                &RemoteAuthMethod::Bearer {
                    token: "secret".into(),
                },
                TransportKind::LoopbackPlaintext,
            ),
            Err(ServerError::AuthenticationFailure)
        ));
        let context = verifier
            .verify(
                &RemoteAuthMethod::Loopback,
                TransportKind::LoopbackPlaintext,
            )
            .expect("loopback");
        assert_eq!(context.tenant_scope(), "tenant-a");
        assert_eq!(context.principal(), "loopback");
    }
}
