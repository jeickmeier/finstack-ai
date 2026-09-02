//! Rust reconnect helper that enforces snapshot-tail-barrier-live order.

use finstack_ai_protocol::{
    POST_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1, RemoteAuthMethod, RemoteCommand,
    RemoteCommandResult, RemoteDurableStep, RemoteEventView, RemoteLocator, RemotePostAuth,
    RemotePreAuth, VersionOffer, require_features, select_version,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ServerError;
use crate::frame::{read_post_auth, read_pre_auth, write_post_auth, write_pre_auth};
use crate::session::ReconnectView;

/// Client-side remote session.
pub struct RemoteClient<S> {
    stream: S,
    post_auth_ceiling: usize,
    barrier_seen: bool,
    protocol_version: u16,
    locator: Option<RemoteLocator>,
    durable_cursor: u64,
    last_transient_sequence: Option<u64>,
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
            protocol_version: PROTOCOL_VERSION_V1,
            locator: None,
            durable_cursor: 0,
            last_transient_sequence: None,
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
    #[expect(
        clippy::too_many_lines,
        reason = "the reconnect protocol state machine is intentionally linear and fail-closed"
    )]
    pub async fn reconnect(
        &mut self,
        offer: &VersionOffer,
        method: RemoteAuthMethod,
        locator: RemoteLocator,
        tenant_scope: impl Into<String>,
        last_known_durable_sequence: Option<u64>,
    ) -> Result<ReconnectView, ServerError> {
        self.barrier_seen = false;
        self.last_transient_sequence = None;
        let expected_locator = locator.clone();
        self.locator = Some(expected_locator.clone());
        write_pre_auth(
            &mut self.stream,
            &RemotePreAuth::ClientHello {
                offer: offer.clone(),
            },
        )
        .await?;
        let RemotePreAuth::ServerHello {
            selected_version,
            offer: server_offer,
        } = read_pre_auth(&mut self.stream).await?
        else {
            return Err(ServerError::AuthenticationFailure);
        };
        if selected_version != select_version(offer, &server_offer)? {
            return Err(codec_error("invalid server version selection"));
        }
        require_features(&server_offer, &["auth"])?;
        self.protocol_version = selected_version;
        write_pre_auth(&mut self.stream, &RemotePreAuth::Authenticate { method }).await?;
        let RemotePreAuth::AuthResult { accepted: true, .. } =
            read_pre_auth(&mut self.stream).await?
        else {
            return Err(ServerError::AuthenticationFailure);
        };
        write_post_auth(
            &mut self.stream,
            self.post_auth_ceiling,
            self.protocol_version,
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
        let mut saw_snapshot_choice = false;
        let mut cursor = 0_u64;
        loop {
            match read_post_auth(
                &mut self.stream,
                self.post_auth_ceiling,
                self.protocol_version,
            )
            .await?
            {
                RemotePostAuth::Snapshot {
                    sequence,
                    snapshot: value,
                } => {
                    if saw_snapshot_choice
                        || value.sequence() != sequence
                        || value.locator() != &expected_locator
                    {
                        return Err(codec_error("invalid reconnect snapshot"));
                    }
                    saw_snapshot_choice = true;
                    snapshot_sequence = sequence;
                    cursor = sequence;
                    snapshot = Some(*value);
                }
                RemotePostAuth::NoSnapshot { sequence } => {
                    if saw_snapshot_choice {
                        return Err(codec_error("duplicate reconnect snapshot choice"));
                    }
                    saw_snapshot_choice = true;
                    snapshot_sequence = sequence;
                    cursor = sequence;
                }
                RemotePostAuth::DurableTail {
                    from_sequence,
                    to_sequence,
                    steps,
                } => {
                    if !saw_snapshot_choice
                        || from_sequence != cursor.saturating_add(1)
                        || steps.first().map(RemoteDurableStep::sequence) != Some(from_sequence)
                        || steps.last().map(RemoteDurableStep::sequence) != Some(to_sequence)
                    {
                        return Err(codec_error("invalid durable tail range"));
                    }
                    for step in &steps {
                        let expected = cursor
                            .checked_add(1)
                            .ok_or_else(|| codec_error("durable cursor exhausted"))?;
                        if step.sequence() != expected {
                            return Err(codec_error("durable tail gap"));
                        }
                        if step.events().iter().any(|event| {
                            !event_matches_locator(event, &expected_locator)
                                || event.durable_sequence() != Some(expected)
                        }) {
                            return Err(codec_error("durable event locator mismatch"));
                        }
                        cursor = expected;
                    }
                    tail.extend(steps);
                }
                RemotePostAuth::SyncBarrier { sequence } => {
                    if !saw_snapshot_choice || sequence != cursor {
                        return Err(codec_error("invalid sync barrier"));
                    }
                    self.barrier_seen = true;
                    self.durable_cursor = sequence;
                    return Ok(ReconnectView {
                        snapshot,
                        snapshot_sequence,
                        tail,
                        barrier: sequence,
                    });
                }
                RemotePostAuth::EventBatch { .. } | RemotePostAuth::Grant { .. } => {
                    return Err(ServerError::LiveBeforeBarrier);
                }
                RemotePostAuth::Close { reason_code } => {
                    return Err(codec_error(reason_code));
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
        let message = read_post_auth(
            &mut self.stream,
            self.post_auth_ceiling,
            self.protocol_version,
        )
        .await?;
        match &message {
            RemotePostAuth::EventBatch { events } => {
                if !self.barrier_seen {
                    return Err(ServerError::LiveBeforeBarrier);
                }
                let locator = self
                    .locator
                    .as_ref()
                    .ok_or_else(|| codec_error("live event without locator"))?;
                for event in events {
                    if event.durable_sequence().is_some()
                        || !event_matches_locator(event, locator)
                        || self
                            .last_transient_sequence
                            .is_some_and(|value| event.transient_sequence() <= value)
                    {
                        return Err(codec_error("invalid live event sequence"));
                    }
                    self.last_transient_sequence = Some(event.transient_sequence());
                }
            }
            RemotePostAuth::Grant { .. } if !self.barrier_seen => {
                return Err(ServerError::LiveBeforeBarrier);
            }
            _ => {}
        }
        Ok(message)
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
        let command_id = command.command_id();
        let digest = command.digest();
        write_post_auth(
            &mut self.stream,
            self.post_auth_ceiling,
            self.protocol_version,
            &RemotePostAuth::Command { command },
        )
        .await?;
        loop {
            match self.next_post_auth().await? {
                RemotePostAuth::CommandResult { result }
                    if result.command_id() == command_id && result.digest() == digest =>
                {
                    let cursor_valid = if result.accepted() {
                        result.durable_sequence() >= self.durable_cursor
                    } else {
                        result.durable_sequence() == self.durable_cursor
                    };
                    if !cursor_valid {
                        return Err(codec_error("invalid command receipt cursor"));
                    }
                    self.durable_cursor = result.durable_sequence();
                    return Ok(result);
                }
                RemotePostAuth::CommandResult { .. } => {
                    return Err(codec_error("unrelated command receipt"));
                }
                RemotePostAuth::EventBatch { .. } | RemotePostAuth::Grant { .. } => {}
                other => {
                    return Err(codec_error(format!("unexpected {}", other.kind())));
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
            self.protocol_version,
            &RemotePostAuth::Ack {
                cursor,
                items,
                bytes,
            },
        )
        .await
    }
}

fn codec_error(message: impl Into<String>) -> ServerError {
    ServerError::Protocol(finstack_ai_protocol::ProtocolError::codec(message))
}

fn event_matches_locator(event: &RemoteEventView, locator: &RemoteLocator) -> bool {
    let event = event.event();
    event.session_id() == locator.session_id()
        && locator
            .lane_id()
            .is_none_or(|lane_id| lane_id == event.lane_id())
        && locator
            .run_id()
            .is_none_or(|run_id| run_id == event.run_id())
}
