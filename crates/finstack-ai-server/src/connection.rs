//! Authenticated connection state machine.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_protocol::{
    POST_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1, RemoteCommand, RemoteDurableStep,
    RemotePostAuth, VersionOffer, encode,
};
use finstack_ai_runtime::{SecurityAuditCategory, SecurityAuditGate};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::ServerError;
use crate::auth::{AuthContext, AuthVerifier, TransportKind};
use crate::credit::CreditWindow;
use crate::frame::{read_post_auth, write_post_auth};
use crate::handshake::{audit_digest, audit_only, handshake};
use crate::session::{ReconnectView, SessionHub};

/// Connection-level limits.
#[derive(Debug, Clone)]
pub(crate) struct ConnectionLimits {
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
pub(crate) async fn serve_connection<S>(
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

    Box::pin(post_auth(
        &mut stream,
        &ctx,
        audit.as_ref(),
        hub.as_ref(),
        &limits,
        credit,
        connection_id,
    ))
    .await
}

async fn post_auth<S>(
    stream: &mut S,
    auth: &AuthContext,
    audit: &SecurityAuditGate,
    hub: &SessionHub,
    limits: &ConnectionLimits,
    credit: CreditWindow,
    connection_id: u64,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let open = read_post_auth(stream, limits.post_auth_ceiling, auth.protocol_version()).await?;
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
    let session_id = locator.session_id().to_string();
    if tenant_scope != auth.tenant_scope() {
        audit_digest(
            audit,
            SecurityAuditCategory::ScopeMismatch,
            "scope_mismatch",
            Some(&session_id),
            None,
        )
        .await?;
        return Err(ServerError::UnknownLocator);
    }

    let plan = match hub.with(&session_id, |replica| {
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
            audit_digest(audit, category, err.code(), Some(&session_id), None).await?;
            return Err(ServerError::UnknownLocator);
        }
    };

    Box::pin(emit_reconnect_plan(
        stream,
        hub,
        limits,
        &credit,
        auth.protocol_version(),
        &session_id,
        plan,
    ))
    .await?;
    serve_post_barrier(
        stream,
        auth,
        audit,
        hub,
        limits,
        credit,
        connection_id,
        &session_id,
    )
    .await
}

async fn emit_reconnect_plan<S>(
    stream: &mut S,
    hub: &SessionHub,
    limits: &ConnectionLimits,
    credit: &CreditWindow,
    protocol_version: u16,
    session_id: &str,
    plan: ReconnectView,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Some(snapshot) = plan.snapshot {
        write_post_auth(
            stream,
            limits.post_auth_ceiling,
            protocol_version,
            &RemotePostAuth::Snapshot {
                sequence: snapshot.sequence(),
                snapshot: Box::new(snapshot),
            },
        )
        .await?;
    } else {
        write_post_auth(
            stream,
            limits.post_auth_ceiling,
            protocol_version,
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
            .map_or(plan.snapshot_sequence + 1, RemoteDurableStep::sequence);
        write_post_auth(
            stream,
            limits.post_auth_ceiling,
            protocol_version,
            &RemotePostAuth::DurableTail {
                from_sequence,
                to_sequence: plan.barrier,
                steps: plan.tail,
            },
        )
        .await?;
    }
    write_post_auth(
        stream,
        limits.post_auth_ceiling,
        protocol_version,
        &RemotePostAuth::SyncBarrier {
            sequence: plan.barrier,
        },
    )
    .await?;
    let _ = hub.with(session_id, |replica| {
        replica.release_barrier();
        Ok(())
    });
    write_post_auth(
        stream,
        limits.post_auth_ceiling,
        protocol_version,
        &RemotePostAuth::Grant {
            items: credit.items(),
            bytes: credit.bytes(),
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn serve_post_barrier<S>(
    stream: &mut S,
    auth: &AuthContext,
    audit: &SecurityAuditGate,
    hub: &SessionHub,
    limits: &ConnectionLimits,
    mut credit: CreditWindow,
    connection_id: u64,
    session_id: &str,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        if credit.items() == 0 {
            match tokio::time::timeout(
                credit.limits().ack_deadline,
                read_post_auth(stream, limits.post_auth_ceiling, auth.protocol_version()),
            )
            .await
            {
                Ok(Ok(RemotePostAuth::Ack { items, bytes, .. })) => {
                    credit.ack(items, bytes);
                    continue;
                }
                Ok(Ok(RemotePostAuth::Close { .. })) => {
                    release_writer(hub, session_id, connection_id);
                    return Ok(());
                }
                _ => {
                    release_writer(hub, session_id, connection_id);
                    return Err(ServerError::CreditTimeout);
                }
            }
        }
        let live = hub.with(session_id, |replica| {
            Ok(replica.take_live(usize::try_from(credit.items()).unwrap_or(0)))
        })?;
        if !live.is_empty() {
            let items = u32::try_from(live.len()).unwrap_or(u32::MAX);
            let message = RemotePostAuth::EventBatch { events: live };
            let bytes = u32::try_from(encode(&message)?.len()).unwrap_or(u32::MAX);
            if let Err(err) = credit.consume(items, bytes) {
                release_writer(hub, session_id, connection_id);
                return Err(err);
            }
            write_post_auth(
                stream,
                limits.post_auth_ceiling,
                auth.protocol_version(),
                &message,
            )
            .await?;
        }

        let incoming =
            match read_post_auth(stream, limits.post_auth_ceiling, auth.protocol_version()).await {
                Ok(message) => message,
                Err(ServerError::Io(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    release_writer(hub, session_id, connection_id);
                    return Ok(());
                }
                Err(err) => {
                    release_writer(hub, session_id, connection_id);
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
                            auth.protocol_version(),
                            &RemotePostAuth::CommandResult { result },
                        )
                        .await?;
                    }
                    Err(err) => {
                        release_writer(hub, session_id, connection_id);
                        return Err(err);
                    }
                }
            }
            RemotePostAuth::Close { .. } => {
                release_writer(hub, session_id, connection_id);
                return Ok(());
            }
            _ => {
                audit_digest(
                    audit,
                    SecurityAuditCategory::UnknownLocator,
                    "unexpected_post_auth",
                    Some(session_id),
                    None,
                )
                .await?;
                release_writer(hub, session_id, connection_id);
                return Err(ServerError::UnknownLocator);
            }
        }
    }
}

fn release_writer(hub: &SessionHub, session_id: &str, connection_id: u64) {
    let _ = hub.with(session_id, |replica| {
        replica.release_writer(connection_id);
        Ok(())
    });
}

async fn apply_command(
    hub: &SessionHub,
    auth: &AuthContext,
    audit: &SecurityAuditGate,
    command: &RemoteCommand,
) -> Result<finstack_ai_protocol::RemoteCommandResult, ServerError> {
    let session_id = command.locator().session_id().to_string();
    match hub.with(&session_id, |replica| replica.apply_command(auth, command)) {
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
                Some(&session_id),
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
