//! Authenticated connection state machine.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_protocol::{
    POST_AUTH_FRAME_MAX_BYTES, RemoteCommand, RemoteDurableStep, RemotePostAuth, VersionOffer,
    encode,
};
use finstack_ai_runtime::audit::{SecurityAuditCategory, SecurityAuditGate};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::ServerError;
use crate::auth::{AuthContext, AuthVerifier, TransportKind};
use crate::credit::CreditWindow;
use crate::frame::{read_post_auth, write_post_auth};
use crate::handshake::{audit_digest, audit_only, handshake};
use crate::session::{ReconnectView, SessionHub};

/// Hello/auth deadline.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);
/// Maximum authenticate attempts per connection.
pub(crate) const AUTH_ATTEMPTS: u8 = 3;
/// Features every client hello must advertise.
pub(crate) const MANDATORY_FEATURES: &[&str] = &["auth"];

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
    offer: &VersionOffer,
    credit: CreditWindow,
    connection_id: u64,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ctx = timeout(
        HANDSHAKE_DEADLINE,
        handshake(&mut stream, transport, auth.as_ref(), audit.as_ref(), offer),
    )
    .await
    .map_err(|_| ServerError::HandshakeTimeout)??;

    Box::pin(post_auth(
        &mut stream,
        &ctx,
        audit.as_ref(),
        hub.as_ref(),
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
    credit: CreditWindow,
    connection_id: u64,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let open = read_post_auth(stream, POST_AUTH_FRAME_MAX_BYTES, auth.protocol_version()).await?;
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
        let plan = replica.plan_reconnect(auth, &locator, last_known_durable_sequence)?;
        replica.claim_writer(connection_id)?;
        Ok(plan)
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

    let _writer = WriterLease {
        hub,
        session_id: &session_id,
        connection_id,
    };
    Box::pin(emit_reconnect_plan(
        stream,
        hub,
        &credit,
        auth.protocol_version(),
        &session_id,
        plan,
    ))
    .await?;
    serve_post_barrier(stream, auth, audit, hub, credit, &session_id).await
}

async fn emit_reconnect_plan<S>(
    stream: &mut S,
    hub: &SessionHub,
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
            POST_AUTH_FRAME_MAX_BYTES,
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
            POST_AUTH_FRAME_MAX_BYTES,
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
            POST_AUTH_FRAME_MAX_BYTES,
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
        POST_AUTH_FRAME_MAX_BYTES,
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
        POST_AUTH_FRAME_MAX_BYTES,
        protocol_version,
        &RemotePostAuth::Grant {
            items: credit.items(),
            bytes: credit.bytes(),
        },
    )
    .await
}

async fn serve_post_barrier<S>(
    stream: &mut S,
    auth: &AuthContext,
    audit: &SecurityAuditGate,
    hub: &SessionHub,
    mut credit: CreditWindow,
    session_id: &str,
) -> Result<(), ServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ready = hub.with(session_id, |replica| Ok(replica.live_ready()))?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    loop {
        // Keep the frame read alive across outbound wakeups: cancelling read_exact
        // after a partial header would lose the frame boundary.
        let incoming = read_post_auth(
            &mut reader,
            POST_AUTH_FRAME_MAX_BYTES,
            auth.protocol_version(),
        );
        tokio::pin!(incoming);
        let incoming = loop {
            if credit.items() == 0 {
                match timeout(credit.limits().ack_deadline, &mut incoming).await {
                    Ok(Ok(RemotePostAuth::Ack { items, bytes, .. })) => {
                        credit.ack(items, bytes);
                        break None;
                    }
                    Ok(Ok(RemotePostAuth::Close { .. })) => return Ok(()),
                    _ => return Err(ServerError::CreditTimeout),
                }
            }
            let notified = ready.notified();
            let live = hub.with(session_id, |replica| {
                Ok(replica.take_live(usize::try_from(credit.items()).unwrap_or(0)))
            })?;
            if !live.is_empty() {
                let items = u32::try_from(live.len()).unwrap_or(u32::MAX);
                let message = RemotePostAuth::EventBatch { events: live };
                let bytes = u32::try_from(encode(&message)?.len()).unwrap_or(u32::MAX);
                credit.consume(items, bytes)?;
                write_post_auth(
                    &mut writer,
                    POST_AUTH_FRAME_MAX_BYTES,
                    auth.protocol_version(),
                    &message,
                )
                .await?;
                continue;
            }
            tokio::select! {
                result = &mut incoming => match result {
                    Ok(message) => break Some(message),
                    Err(ServerError::Io(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                    Err(err) => return Err(err),
                },
                () = notified => {}
            }
        };
        let Some(incoming) = incoming else {
            continue;
        };
        match incoming {
            RemotePostAuth::Ack { items, bytes, .. } => credit.ack(items, bytes),
            RemotePostAuth::Command { command } => {
                match apply_command(hub, auth, audit, &command).await {
                    Ok(result) => {
                        write_post_auth(
                            &mut writer,
                            POST_AUTH_FRAME_MAX_BYTES,
                            auth.protocol_version(),
                            &RemotePostAuth::CommandResult { result },
                        )
                        .await?;
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            RemotePostAuth::Close { .. } => {
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
                return Err(ServerError::UnknownLocator);
            }
        }
    }
}

struct WriterLease<'a> {
    hub: &'a SessionHub,
    session_id: &'a str,
    connection_id: u64,
}

impl Drop for WriterLease<'_> {
    fn drop(&mut self) {
        release_writer(self.hub, self.session_id, self.connection_id);
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
