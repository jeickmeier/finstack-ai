//! Authenticated connection state machine.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{Digest, Timestamp};
use finstack_ai_protocol::{
    POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1, PayloadFamily,
    ProtocolEnvelope, RemoteAuthMethod, RemoteCommand, RemoteEventView, RemotePostAuth,
    RemotePreAuth, VersionOffer, decode_envelope, encode_envelope, require_features,
    select_version,
};
use finstack_ai_runtime::{SecurityAuditCategory, SecurityAuditEvent, SecurityAuditGate};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;
use uuid::Uuid;

use crate::ServerError;
use crate::auth::{AuthContext, AuthVerifier, TransportKind};
use crate::io::{read_frame, write_frame};
use crate::replica::{CreditWindow, SessionHub};

/// Connection-level limits.
#[derive(Debug, Clone)]
pub struct ConnectionLimits {
    /// Hello/auth deadline.
    pub handshake_deadline: Duration,
    /// Maximum authenticate attempts.
    pub auth_attempts: u8,
    /// Mandatory hello features.
    pub mandatory_features: Vec<String>,
    /// Server version offer.
    pub offer: VersionOffer,
    /// Post-auth frame ceiling.
    pub post_auth_ceiling: usize,
}

impl ConnectionLimits {
    /// Construct default v1 limits.
    ///
    /// # Errors
    ///
    /// Returns a protocol error when the default offer is invalid.
    pub fn v1() -> Result<Self, ServerError> {
        Ok(Self {
            handshake_deadline: Duration::from_secs(5),
            auth_attempts: 3,
            mandatory_features: vec!["auth".into()],
            offer: VersionOffer::try_new(
                vec![PROTOCOL_VERSION_V1],
                PROTOCOL_VERSION_V1,
                vec!["auth".into()],
            )?,
            post_auth_ceiling: POST_AUTH_FRAME_MAX_BYTES,
        })
    }
}

/// Serve one accepted stream through hello, auth, reconnect, and commands.
///
/// # Errors
///
/// Returns the first fail-closed protocol, auth, or I/O error.
#[allow(clippy::too_many_arguments)]
pub async fn serve_connection<S>(
    mut stream: S,
    transport: TransportKind,
    auth: Arc<dyn AuthVerifier>,
    audit: Arc<SecurityAuditGate>,
    hub: Arc<SessionHub>,
    limits: ConnectionLimits,
    credit: CreditWindow,
    connection_id: u64,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ctx = timeout(limits.handshake_deadline, async {
        handshake(
            &mut stream,
            transport,
            auth.as_ref(),
            audit.as_ref(),
            &limits,
        )
        .await
    })
    .await
    .map_err(|_| ServerError::HandshakeTimeout)??;

    post_auth(
        &mut stream,
        &ctx,
        audit.as_ref(),
        hub.as_ref(),
        &limits,
        credit,
        connection_id,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn handshake<S>(
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
                        return Ok(ctx);
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

#[allow(clippy::too_many_lines)]
async fn post_auth<S>(
    stream: &mut S,
    auth: &AuthContext,
    audit: &SecurityAuditGate,
    hub: &SessionHub,
    limits: &ConnectionLimits,
    mut credit: CreditWindow,
    connection_id: u64,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let open = read_post_auth(stream, limits.post_auth_ceiling).await?;
    let RemotePostAuth::OpenSession {
        last_known_durable_sequence,
        locator,
        tenant_scope,
    } = open
    else {
        audit_only(
            audit,
            SecurityAuditCategory::UnknownLocator,
            "expected_open",
            None,
        )
        .await?;
        return Err(ServerError::UnknownLocator);
    };
    if tenant_scope != auth.tenant_scope() {
        audit_digest(
            audit,
            SecurityAuditCategory::ScopeMismatch,
            "scope_mismatch",
            Some(locator.session_id()),
            None,
        )
        .await?;
        return Err(ServerError::UnknownLocator);
    }

    let plan = match hub.with(locator.session_id(), |replica| {
        replica.claim_writer(connection_id)?;
        replica.plan_reconnect(auth, &locator, last_known_durable_sequence)
    }) {
        Ok(plan) => plan,
        Err(err) => {
            let category = match err {
                ServerError::ScopeMismatch | ServerError::SessionBusy => {
                    SecurityAuditCategory::ScopeMismatch
                }
                _ => SecurityAuditCategory::UnknownLocator,
            };
            audit_digest(
                audit,
                category,
                err.code(),
                Some(locator.session_id()),
                None,
            )
            .await?;
            return Err(ServerError::UnknownLocator);
        }
    };

    if let Some(snapshot) = plan.snapshot.clone() {
        write_post_auth(
            stream,
            limits.post_auth_ceiling,
            &RemotePostAuth::Snapshot {
                sequence: snapshot.sequence(),
                snapshot,
            },
        )
        .await?;
    } else {
        write_post_auth(
            stream,
            limits.post_auth_ceiling,
            &RemotePostAuth::NoSnapshot {
                sequence: plan.snapshot_sequence,
            },
        )
        .await?;
    }
    if !plan.tail.is_empty() {
        let from_sequence = plan
            .tail
            .first()
            .and_then(RemoteEventView::durable_sequence)
            .unwrap_or(plan.snapshot_sequence + 1);
        write_post_auth(
            stream,
            limits.post_auth_ceiling,
            &RemotePostAuth::DurableTail {
                from_sequence,
                to_sequence: plan.barrier,
                events: plan.tail.clone(),
            },
        )
        .await?;
    }
    write_post_auth(
        stream,
        limits.post_auth_ceiling,
        &RemotePostAuth::SyncBarrier {
            sequence: plan.barrier,
        },
    )
    .await?;
    let _ = hub.with(locator.session_id(), |replica| {
        replica.release_barrier();
        Ok(())
    });

    write_post_auth(
        stream,
        limits.post_auth_ceiling,
        &RemotePostAuth::Grant {
            items: credit.items(),
            bytes: credit.bytes(),
        },
    )
    .await?;

    loop {
        if credit.items() == 0 {
            match tokio::time::timeout(
                credit.limits().ack_deadline,
                read_post_auth(stream, limits.post_auth_ceiling),
            )
            .await
            {
                Ok(Ok(RemotePostAuth::Ack { items, bytes, .. })) => {
                    credit.ack(items, bytes);
                    continue;
                }
                Ok(Ok(RemotePostAuth::Close { .. })) => {
                    let _ = hub.with(locator.session_id(), |replica| {
                        replica.release_writer(connection_id);
                        Ok(())
                    });
                    return Ok(());
                }
                _ => {
                    let _ = hub.with(locator.session_id(), |replica| {
                        replica.release_writer(connection_id);
                        Ok(())
                    });
                    return Err(ServerError::CreditTimeout);
                }
            }
        }
        let live = hub.with(locator.session_id(), |replica| {
            Ok(replica.take_live(usize::try_from(credit.items()).unwrap_or(0)))
        })?;
        if !live.is_empty() {
            let items = u32::try_from(live.len()).unwrap_or(u32::MAX);
            let bytes = u32::try_from(
                live.iter()
                    .map(|event| event.event_id().len() + event.kind().len())
                    .sum::<usize>(),
            )
            .unwrap_or(u32::MAX);
            if let Err(err) = credit.consume(items, bytes) {
                let _ = hub.with(locator.session_id(), |replica| {
                    replica.release_writer(connection_id);
                    Ok(())
                });
                return Err(err);
            }
            write_post_auth(
                stream,
                limits.post_auth_ceiling,
                &RemotePostAuth::EventBatch {
                    live: true,
                    events: live,
                },
            )
            .await?;
        }

        let incoming = match read_post_auth(stream, limits.post_auth_ceiling).await {
            Ok(message) => message,
            Err(ServerError::Io(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                let _ = hub.with(locator.session_id(), |replica| {
                    replica.release_writer(connection_id);
                    Ok(())
                });
                return Ok(());
            }
            Err(err) => {
                let _ = hub.with(locator.session_id(), |replica| {
                    replica.release_writer(connection_id);
                    Ok(())
                });
                return Err(err);
            }
        };
        match incoming {
            RemotePostAuth::Ack { items, bytes, .. } => credit.ack(items, bytes),
            RemotePostAuth::Command { command } => {
                match apply_command(hub, auth, audit, &command).await {
                    Ok(result) => {
                        write_post_auth(
                            stream,
                            limits.post_auth_ceiling,
                            &RemotePostAuth::CommandResult { result },
                        )
                        .await?;
                    }
                    Err(err) => {
                        let _ = hub.with(locator.session_id(), |replica| {
                            replica.release_writer(connection_id);
                            Ok(())
                        });
                        return Err(err);
                    }
                }
            }
            RemotePostAuth::Close { .. } => {
                let _ = hub.with(locator.session_id(), |replica| {
                    replica.release_writer(connection_id);
                    Ok(())
                });
                return Ok(());
            }
            _ => {
                audit_digest(
                    audit,
                    SecurityAuditCategory::UnknownLocator,
                    "unexpected_post_auth",
                    Some(locator.session_id()),
                    None,
                )
                .await?;
                let _ = hub.with(locator.session_id(), |replica| {
                    replica.release_writer(connection_id);
                    Ok(())
                });
                return Err(ServerError::UnknownLocator);
            }
        }
    }
}

async fn apply_command(
    hub: &SessionHub,
    auth: &AuthContext,
    audit: &SecurityAuditGate,
    command: &RemoteCommand,
) -> Result<finstack_ai_protocol::RemoteCommandResult, ServerError> {
    match hub.with(command.locator().session_id(), |replica| {
        replica.apply_command(auth, command)
    }) {
        Ok(result) => Ok(result),
        Err(err) => {
            let category = match err {
                ServerError::ScopeMismatch | ServerError::IdempotencyConflict => {
                    SecurityAuditCategory::ScopeMismatch
                }
                _ => SecurityAuditCategory::UnknownLocator,
            };
            audit_digest(
                audit,
                category,
                err.code(),
                Some(command.locator().session_id()),
                Some(command.digest()),
            )
            .await?;
            Err(if matches!(err, ServerError::ScopeMismatch) {
                ServerError::UnknownLocator
            } else {
                err
            })
        }
    }
}

async fn read_pre_auth<S: AsyncRead + Unpin>(stream: &mut S) -> Result<RemotePreAuth, ServerError> {
    let payload = read_frame(stream, PRE_AUTH_FRAME_MAX_BYTES).await?;
    let envelope: ProtocolEnvelope<RemotePreAuth> =
        decode_envelope(&payload, PayloadFamily::Remote)?;
    Ok(envelope.into_body())
}

async fn write_pre_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    body: &RemotePreAuth,
) -> Result<(), ServerError> {
    let payload = encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body)?;
    write_frame(stream, &payload, PRE_AUTH_FRAME_MAX_BYTES).await
}

async fn read_post_auth<S: AsyncRead + Unpin>(
    stream: &mut S,
    ceiling: usize,
) -> Result<RemotePostAuth, ServerError> {
    let payload = read_frame(stream, ceiling).await?;
    let envelope: ProtocolEnvelope<RemotePostAuth> =
        decode_envelope(&payload, PayloadFamily::Remote)?;
    Ok(envelope.into_body())
}

async fn write_post_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    ceiling: usize,
    body: &RemotePostAuth,
) -> Result<(), ServerError> {
    let payload = encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body)?;
    write_frame(stream, &payload, ceiling).await
}

async fn audit_only(
    gate: &SecurityAuditGate,
    category: SecurityAuditCategory,
    reason_code: &str,
    submission: Option<Digest>,
) -> Result<(), ServerError> {
    audit_digest(gate, category, reason_code, None, submission).await
}

async fn audit_digest(
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

fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    Timestamp::from_unix_ms(ms).unwrap_or_else(|_| Timestamp::from_unix_ms(0).expect("epoch"))
}
