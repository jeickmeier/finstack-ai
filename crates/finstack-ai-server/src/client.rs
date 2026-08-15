//! Rust reconnect helper that enforces snapshot-tail-barrier-live order.

use finstack_ai_protocol::{
    POST_AUTH_FRAME_MAX_BYTES, RemoteAuthMethod, RemoteCommand, RemoteCommandResult,
    RemoteEventView, RemoteLocator, RemotePostAuth, RemotePreAuth, RemoteSnapshot, VersionOffer,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ServerError;
use crate::io::{read_post_auth, read_pre_auth, write_post_auth, write_pre_auth};

/// Messages received after a successful reconnect barrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectView {
    /// Snapshot at `S`, when present.
    pub snapshot: Option<RemoteSnapshot>,
    /// Explicit no-snapshot sequence when `snapshot` is `None`.
    pub snapshot_sequence: u64,
    /// Durable tail events.
    pub tail: Vec<RemoteEventView>,
    /// Barrier sequence.
    pub barrier: u64,
}

/// Client-side remote session.
pub struct RemoteClient<S> {
    stream: S,
    post_auth_ceiling: usize,
    barrier_seen: bool,
}

impl<S> RemoteClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Wrap an already-connected stream.
    #[must_use]
    pub const fn new(stream: S) -> Self {
        Self {
            stream,
            post_auth_ceiling: POST_AUTH_FRAME_MAX_BYTES,
            barrier_seen: false,
        }
    }

    /// Authenticate and open a session, returning the reconnect plan.
    ///
    /// Live `EventBatch` messages before [`RemotePostAuth::SyncBarrier`] fail
    /// the client.
    ///
    /// # Errors
    ///
    /// Returns protocol, auth, or ordering failures.
    pub async fn reconnect(
        &mut self,
        offer: &VersionOffer,
        method: RemoteAuthMethod,
        locator: RemoteLocator,
        tenant_scope: impl Into<String>,
        last_known_durable_sequence: Option<u64>,
    ) -> Result<ReconnectView, ServerError> {
        self.barrier_seen = false;
        write_pre_auth(
            &mut self.stream,
            &RemotePreAuth::ClientHello {
                offer: offer.clone(),
            },
        )
        .await?;
        let RemotePreAuth::ServerHello { .. } = read_pre_auth(&mut self.stream).await? else {
            return Err(ServerError::AuthenticationFailure);
        };
        write_pre_auth(&mut self.stream, &RemotePreAuth::Authenticate { method }).await?;
        let RemotePreAuth::AuthResult { accepted: true, .. } =
            read_pre_auth(&mut self.stream).await?
        else {
            return Err(ServerError::AuthenticationFailure);
        };
        write_post_auth(
            &mut self.stream,
            self.post_auth_ceiling,
            &RemotePostAuth::OpenSession {
                last_known_durable_sequence,
                locator,
                tenant_scope: tenant_scope.into(),
            },
        )
        .await?;

        let mut snapshot = None;
        let mut snapshot_sequence = 0;
        let mut tail = Vec::new();
        loop {
            match read_post_auth(&mut self.stream, self.post_auth_ceiling).await? {
                RemotePostAuth::Snapshot {
                    sequence,
                    snapshot: value,
                } => {
                    snapshot_sequence = sequence;
                    snapshot = Some(value);
                }
                RemotePostAuth::NoSnapshot { sequence } => snapshot_sequence = sequence,
                RemotePostAuth::DurableTail { events, .. } => tail = events,
                RemotePostAuth::SyncBarrier { sequence } => {
                    self.barrier_seen = true;
                    return Ok(ReconnectView {
                        snapshot,
                        snapshot_sequence,
                        tail,
                        barrier: sequence,
                    });
                }
                RemotePostAuth::EventBatch { live: true, .. } | RemotePostAuth::Grant { .. }
                    if !self.barrier_seen =>
                {
                    return Err(ServerError::LiveBeforeBarrier);
                }
                RemotePostAuth::Close { reason_code } => {
                    return Err(ServerError::Protocol(
                        finstack_ai_protocol::ProtocolError::codec(reason_code),
                    ));
                }
                _ => return Err(ServerError::UnknownLocator),
            }
        }
    }

    /// Read the next post-auth message after the barrier.
    ///
    /// # Errors
    ///
    /// Returns protocol or I/O failures.
    pub async fn next_post_auth(&mut self) -> Result<RemotePostAuth, ServerError> {
        read_post_auth(&mut self.stream, self.post_auth_ceiling).await
    }

    /// Send a command and wait for its result.
    ///
    /// # Errors
    ///
    /// Returns protocol or I/O failures.
    pub async fn command(
        &mut self,
        command: RemoteCommand,
    ) -> Result<RemoteCommandResult, ServerError> {
        write_post_auth(
            &mut self.stream,
            self.post_auth_ceiling,
            &RemotePostAuth::Command { command },
        )
        .await?;
        loop {
            match self.next_post_auth().await? {
                RemotePostAuth::CommandResult { result } => return Ok(result),
                RemotePostAuth::EventBatch { .. } | RemotePostAuth::Grant { .. } => {}
                other => {
                    return Err(ServerError::Protocol(
                        finstack_ai_protocol::ProtocolError::codec(format!(
                            "unexpected {}",
                            message_kind(&other)
                        )),
                    ));
                }
            }
        }
    }

    /// Acknowledge consumed credits.
    ///
    /// # Errors
    ///
    /// Returns protocol or I/O failures.
    pub async fn ack(&mut self, cursor: u64, items: u32, bytes: u32) -> Result<(), ServerError> {
        write_post_auth(
            &mut self.stream,
            self.post_auth_ceiling,
            &RemotePostAuth::Ack {
                cursor,
                items,
                bytes,
            },
        )
        .await
    }
}

fn message_kind(message: &RemotePostAuth) -> &'static str {
    match message {
        RemotePostAuth::OpenSession { .. } => "open_session",
        RemotePostAuth::Snapshot { .. } => "snapshot",
        RemotePostAuth::NoSnapshot { .. } => "no_snapshot",
        RemotePostAuth::DurableTail { .. } => "durable_tail",
        RemotePostAuth::SyncBarrier { .. } => "sync_barrier",
        RemotePostAuth::EventBatch { .. } => "event_batch",
        RemotePostAuth::Command { .. } => "command",
        RemotePostAuth::CommandResult { .. } => "command_result",
        RemotePostAuth::Grant { .. } => "grant",
        RemotePostAuth::Ack { .. } => "ack",
        RemotePostAuth::Close { .. } => "close",
    }
}
