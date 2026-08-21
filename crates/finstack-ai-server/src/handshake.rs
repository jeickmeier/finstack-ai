//! Pre-auth hello and authenticate, plus shared audit helpers.

use std::time::{SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{Digest, Timestamp};
use finstack_ai_protocol::{RemoteAuthMethod, RemotePreAuth, require_features, select_version};
use finstack_ai_runtime::{SecurityAuditCategory, SecurityAuditEvent, SecurityAuditGate};
use tokio::io::{AsyncRead, AsyncWrite};
use uuid::Uuid;

use crate::ServerError;
use crate::auth::{AuthContext, AuthVerifier, TransportKind};
use crate::connection::ConnectionLimits;
use crate::frame::{read_pre_auth, write_pre_auth};

#[allow(clippy::too_many_lines)]
pub(crate) async fn handshake<S>(
    stream: &mut S,
    transport: TransportKind,
    auth: &dyn AuthVerifier,
    audit: &SecurityAuditGate,
    limits: &ConnectionLimits,
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
            None,
        )
        .await?;
        return Err(ServerError::AuthenticationFailure);
    };
    require_features(
        &offer,
        &limits
            .mandatory_features
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )?;
    let selected = select_version(&offer, &limits.offer)?;
    write_pre_auth(
        stream,
        &RemotePreAuth::ServerHello {
            selected_version: selected,
            offer: limits.offer.clone(),
        },
    )
    .await?;

    let mut attempts = 0_u8;
    loop {
        attempts = attempts.saturating_add(1);
        if attempts > limits.auth_attempts {
            audit_only(
                audit,
                SecurityAuditCategory::AuthenticationFailure,
                "auth_attempt_cap",
                None,
            )
            .await?;
            return Err(ServerError::AuthenticationFailure);
        }
        match read_pre_auth(stream).await? {
            RemotePreAuth::Authenticate { method } => {
                if matches!(method, RemoteAuthMethod::Bearer { .. }) && !transport.allows_bearer() {
                    audit_only(
                        audit,
                        SecurityAuditCategory::AuthenticationFailure,
                        "bearer_over_plaintext",
                        None,
                    )
                    .await?;
                    write_pre_auth(
                        stream,
                        &RemotePreAuth::AuthResult {
                            accepted: false,
                            reason_code: Some("authentication_failure".into()),
                        },
                    )
                    .await?;
                    return Err(ServerError::AuthenticationFailure);
                }
                match auth.verify(&method, transport) {
                    Ok(ctx) => {
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
                    Err(err) => {
                        audit_only(
                            audit,
                            SecurityAuditCategory::AuthenticationFailure,
                            err.code(),
                            None,
                        )
                        .await?;
                        write_pre_auth(
                            stream,
                            &RemotePreAuth::AuthResult {
                                accepted: false,
                                reason_code: Some(err.code().into()),
                            },
                        )
                        .await?;
                    }
                }
            }
            RemotePreAuth::Close { .. } => return Err(ServerError::AuthenticationFailure),
            _ => {
                audit_only(
                    audit,
                    SecurityAuditCategory::AuthenticationFailure,
                    "unexpected_pre_auth",
                    None,
                )
                .await?;
                return Err(ServerError::AuthenticationFailure);
            }
        }
    }
}

/// Record an audit event without a locator digest so authentication
/// failures cannot leak a session id.
pub(crate) async fn audit_only(
    gate: &SecurityAuditGate,
    category: SecurityAuditCategory,
    reason_code: &str,
    submission: Option<Digest>,
) -> Result<(), ServerError> {
    audit_digest(gate, category, reason_code, None, submission).await
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
