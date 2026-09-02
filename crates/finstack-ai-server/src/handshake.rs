//! Pre-auth hello and authenticate, plus shared audit helpers.

use std::time::{SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{Digest, Timestamp};
use finstack_ai_protocol::{
    RemoteAuthMethod, RemotePreAuth, VersionOffer, require_features, select_version,
};
use finstack_ai_runtime::audit::{SecurityAuditCategory, SecurityAuditEvent, SecurityAuditGate};
use tokio::io::{AsyncRead, AsyncWrite};
use uuid::Uuid;

use crate::ServerError;
use crate::auth::{AuthContext, AuthVerifier, TransportKind};
use crate::connection::{AUTH_ATTEMPTS, MANDATORY_FEATURES};
use crate::frame::{read_pre_auth, write_pre_auth};

pub(crate) async fn handshake<S>(
    stream: &mut S,
    transport: TransportKind,
    auth: &dyn AuthVerifier,
    audit: &SecurityAuditGate,
    server_offer: &VersionOffer,
) -> Result<AuthContext, ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello = read_pre_auth(stream).await?;
    let RemotePreAuth::ClientHello { offer } = hello else {
        audit_only(
            audit,
            SecurityAuditCategory::AuthenticationFailure,
            "expected_hello",
        )
        .await?;
        return Err(ServerError::AuthenticationFailure);
    };
    require_features(&offer, MANDATORY_FEATURES)?;
    let selected = select_version(&offer, server_offer)?;
    write_pre_auth(
        stream,
        &RemotePreAuth::ServerHello {
            selected_version: selected,
            offer: server_offer.clone(),
        },
    )
    .await?;

    let mut attempts = 0_u8;
    loop {
        attempts = attempts.saturating_add(1);
        if attempts > AUTH_ATTEMPTS {
            audit_only(
                audit,
                SecurityAuditCategory::AuthenticationFailure,
                "auth_attempt_cap",
            )
            .await?;
            return Err(ServerError::AuthenticationFailure);
        }
        match read_pre_auth(stream).await? {
            RemotePreAuth::Authenticate { method } => {
                let transport_rejection = match &method {
                    RemoteAuthMethod::Bearer { .. } if !transport.allows_bearer() => {
                        Some("bearer_over_plaintext")
                    }
                    RemoteAuthMethod::Loopback if transport != TransportKind::LoopbackPlaintext => {
                        Some("loopback_over_non_loopback_transport")
                    }
                    RemoteAuthMethod::Bearer { token } if token.is_empty() => Some("empty_bearer"),
                    _ => None,
                };
                if let Some(reason_code) = transport_rejection {
                    audit_only(
                        audit,
                        SecurityAuditCategory::AuthenticationFailure,
                        reason_code,
                    )
                    .await?;
                    write_auth_rejected(stream).await?;
                    return Err(ServerError::AuthenticationFailure);
                }
                if let Ok(ctx) = auth.verify(&method, transport) {
                    write_pre_auth(
                        stream,
                        &RemotePreAuth::AuthResult {
                            accepted: true,
                            reason_code: None,
                        },
                    )
                    .await?;
                    return Ok(ctx.with_protocol_version(selected));
                }
                audit_only(
                    audit,
                    SecurityAuditCategory::AuthenticationFailure,
                    "authentication_failure",
                )
                .await?;
                write_auth_rejected(stream).await?;
            }
            RemotePreAuth::Close { .. } => return Err(ServerError::AuthenticationFailure),
            _ => {
                audit_only(
                    audit,
                    SecurityAuditCategory::AuthenticationFailure,
                    "unexpected_pre_auth",
                )
                .await?;
                return Err(ServerError::AuthenticationFailure);
            }
        }
    }
}

async fn write_auth_rejected<S: AsyncWrite + Unpin>(stream: &mut S) -> Result<(), ServerError> {
    write_pre_auth(
        stream,
        &RemotePreAuth::AuthResult {
            accepted: false,
            reason_code: Some("authentication_failure".into()),
        },
    )
    .await
}

/// Record an audit event without a locator digest so authentication
/// failures cannot leak a session id.
pub(crate) async fn audit_only(
    gate: &SecurityAuditGate,
    category: SecurityAuditCategory,
    reason_code: &str,
) -> Result<(), ServerError> {
    audit_digest(gate, category, reason_code, None, None).await
}

pub(crate) async fn audit_digest(
    gate: &SecurityAuditGate,
    category: SecurityAuditCategory,
    reason_code: &str,
    locator: Option<&str>,
    submission: Option<Digest>,
) -> Result<(), ServerError> {
    let locator_digest = locator
        .map(|value| {
            Digest::domain_separated("remote-locator", 1, value.as_bytes()).map_err(|err| {
                ServerError::Protocol(finstack_ai_protocol::ProtocolError::codec(err.to_string()))
            })
        })
        .transpose()?;
    let event = SecurityAuditEvent::try_new(
        Uuid::now_v7().to_string(),
        now(),
        None,
        None::<&str>,
        category,
        reason_code,
        locator_digest,
        submission,
    )
    .map_err(|_| ServerError::AuditNotReady)?;
    gate.record(event).await?;
    Ok(())
}

pub(crate) fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    Timestamp::from_unix_ms(ms).unwrap_or(finstack_ai_kernel::UNIX_EPOCH)
}
