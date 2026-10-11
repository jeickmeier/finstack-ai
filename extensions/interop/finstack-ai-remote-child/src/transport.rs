//! Length-prefixed session-server framing contract client over an explicit stream.

use finstack_ai_kernel::ChildRunLocator;
use finstack_ai_protocol::{
    FRAME_LENGTH_BYTES, POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1,
    PayloadFamily, ProtocolEnvelope, RemoteAuthMethod, RemoteCommand, RemoteCommandPayload,
    RemoteCommandResult, RemoteDurableStep, RemoteEventView, RemoteLocator, RemotePostAuth,
    RemotePreAuth, VersionOffer, decode_envelope, decode_frame_len, encode_envelope, encode_frame,
    require_features, select_version,
};
use finstack_ai_runtime::child::AgentInvokeError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Duration, Instant};

use crate::route::{RemoteEndpoint, ResolvedRemoteRoute, unavailable};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OPEN_FRAMES: usize = 64;
const MAX_COMMAND_FRAMES: usize = 64;

pub(crate) async fn exchange(
    route: &ResolvedRemoteRoute,
    locator: &ChildRunLocator,
    payload: RemoteCommandPayload,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    match &route.endpoint {
        RemoteEndpoint::Tcp(addr) => {
            let connect = tokio::net::TcpStream::connect(*addr);
            exchange_over(connect, route, locator, payload, command_id).await
        }
        #[cfg(unix)]
        RemoteEndpoint::Unix(path) => {
            let connect = tokio::net::UnixStream::connect(path);
            exchange_over(connect, route, locator, payload, command_id).await
        }
        #[cfg(not(unix))]
        RemoteEndpoint::Unix(_) => {
            Err(unavailable("unix remote child endpoints are not supported"))
        }
    }
}

/// Connect within [`CONNECT_TIMEOUT`], then run one exchange within
/// [`EXCHANGE_TIMEOUT`].
async fn exchange_over<S>(
    connect: impl Future<Output = std::io::Result<S>>,
    route: &ResolvedRemoteRoute,
    locator: &ChildRunLocator,
    payload: RemoteCommandPayload,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| unavailable("remote child connect timed out"))?
        .map_err(|error| unavailable(error.to_string()))?;
    tokio::time::timeout(
        EXCHANGE_TIMEOUT,
        exchange_on(stream, route, locator, payload, command_id),
    )
    .await
    .map_err(|_| unavailable("remote child exchange timed out"))?
}

async fn exchange_on<S>(
    mut stream: S,
    route: &ResolvedRemoteRoute,
    locator: &ChildRunLocator,
    payload: RemoteCommandPayload,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let selected_version = authenticate(&mut stream, route).await?;
    let remote_locator = RemoteLocator::try_new(
        locator.operation.session_id,
        Some(locator.operation.lane_id),
        Some(locator.operation.run_id),
    )
    .map_err(|error| unavailable(error.to_string()))?;
    write_post_auth(
        &mut stream,
        selected_version,
        &RemotePostAuth::OpenSession {
            last_known_durable_sequence: None,
            locator: remote_locator.clone(),
            tenant_scope: locator.operation.tenant_scope.to_string(),
        },
    )
    .await?;
    let started = Instant::now();
    let durable_cursor =
        read_until_sync_barrier(&mut stream, selected_version, &remote_locator, started).await?;
    let command = RemoteCommand::try_new(
        command_id,
        remote_locator.clone(),
        locator.operation.tenant_scope.as_ref(),
        durable_cursor,
        payload,
    )
    .map_err(|error| unavailable(error.to_string()))?;
    send_command_for_receipt(
        &mut stream,
        selected_version,
        &remote_locator,
        durable_cursor,
        command,
        started,
    )
    .await
}

/// Negotiate the protocol version and authenticate; returns the selected
/// post-auth protocol version.
async fn authenticate<S>(
    stream: &mut S,
    route: &ResolvedRemoteRoute,
) -> Result<u16, AgentInvokeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let offer = VersionOffer::try_new(
        vec![PROTOCOL_VERSION_V1],
        PROTOCOL_VERSION_V1,
        vec!["auth".into()],
    )
    .map_err(|error| unavailable(error.to_string()))?;
    write_pre_auth(
        stream,
        &RemotePreAuth::ClientHello {
            offer: offer.clone(),
        },
    )
    .await?;
    let RemotePreAuth::ServerHello {
        selected_version,
        offer: server_offer,
    } = read_pre_auth(stream).await?
    else {
        return Err(unavailable("remote child handshake failed"));
    };
    if selected_version
        != select_version(&offer, &server_offer).map_err(|error| unavailable(error.to_string()))?
    {
        return Err(unavailable(
            "remote child server selected the wrong version",
        ));
    }
    require_features(&server_offer, &["auth"]).map_err(|error| unavailable(error.to_string()))?;
    let method = match route
        .token
        .as_ref()
        .map(finstack_ai_runtime::ports::model::SecretString::expose)
    {
        Some(token) if !token.is_empty() => RemoteAuthMethod::Bearer {
            token: token.to_owned(),
        },
        _ => RemoteAuthMethod::Loopback,
    };
    write_pre_auth(stream, &RemotePreAuth::Authenticate { method }).await?;
    let RemotePreAuth::AuthResult { accepted: true, .. } = read_pre_auth(stream).await? else {
        return Err(unavailable("remote child authentication failed"));
    };
    Ok(selected_version)
}

/// Durable catch-up state accumulated while opening a session.
struct OpenProgress<'l> {
    remote_locator: &'l RemoteLocator,
    saw_snapshot_choice: bool,
    durable_cursor: u64,
}

impl OpenProgress<'_> {
    /// Apply one open-phase frame; returns the barrier sequence once the
    /// session reaches its sync barrier.
    fn apply(&mut self, frame: RemotePostAuth) -> Result<Option<u64>, AgentInvokeError> {
        match frame {
            RemotePostAuth::Snapshot { sequence, snapshot } => {
                if self.saw_snapshot_choice
                    || snapshot.sequence() != sequence
                    || snapshot.locator() != self.remote_locator
                {
                    return Err(unavailable("remote child received an invalid snapshot"));
                }
                self.saw_snapshot_choice = true;
                self.durable_cursor = sequence;
            }
            RemotePostAuth::NoSnapshot { sequence } => {
                if self.saw_snapshot_choice {
                    return Err(unavailable(
                        "remote child received duplicate snapshot state",
                    ));
                }
                self.saw_snapshot_choice = true;
                self.durable_cursor = sequence;
            }
            RemotePostAuth::DurableTail {
                from_sequence,
                to_sequence,
                steps,
            } => self.apply_tail(from_sequence, to_sequence, &steps)?,
            RemotePostAuth::SyncBarrier { sequence } => {
                if !self.saw_snapshot_choice || sequence != self.durable_cursor {
                    return Err(unavailable("remote child received the wrong sync barrier"));
                }
                return Ok(Some(sequence));
            }
            RemotePostAuth::EventBatch { .. } => {
                return Err(unavailable(
                    "remote child live events arrived before barrier",
                ));
            }
            other => {
                return Err(unavailable(format!(
                    "remote child open returned unexpected {}",
                    other.kind()
                )));
            }
        }
        Ok(None)
    }

    fn apply_tail(
        &mut self,
        from_sequence: u64,
        to_sequence: u64,
        steps: &[RemoteDurableStep],
    ) -> Result<(), AgentInvokeError> {
        if !self.saw_snapshot_choice || from_sequence != self.durable_cursor.saturating_add(1) {
            return Err(unavailable("remote child received a discontinuous tail"));
        }
        for step in steps {
            self.durable_cursor = self.durable_cursor.saturating_add(1);
            if step.sequence() != self.durable_cursor {
                return Err(unavailable("remote child received a tail gap"));
            }
        }
        if self.durable_cursor != to_sequence {
            return Err(unavailable("remote child received an invalid tail range"));
        }
        Ok(())
    }
}

/// Read the snapshot choice and durable tail up to the sync barrier within
/// [`MAX_OPEN_FRAMES`]; returns the barrier's durable sequence.
async fn read_until_sync_barrier<S>(
    stream: &mut S,
    protocol_version: u16,
    remote_locator: &RemoteLocator,
    started: Instant,
) -> Result<u64, AgentInvokeError>
where
    S: AsyncRead + Unpin,
{
    let mut progress = OpenProgress {
        remote_locator,
        saw_snapshot_choice: false,
        durable_cursor: 0,
    };
    let mut open_frames = 0_usize;
    loop {
        open_frames = open_frames.saturating_add(1);
        if open_frames > MAX_OPEN_FRAMES || started.elapsed() > EXCHANGE_TIMEOUT {
            return Err(unavailable("remote child open exceeded the frame bound"));
        }
        let frame = read_post_auth(stream, protocol_version).await?;
        if let Some(sequence) = progress.apply(frame)? {
            return Ok(sequence);
        }
    }
}

/// Send `command` and read until its matching receipt within
/// [`MAX_COMMAND_FRAMES`], validating interleaved live events.
async fn send_command_for_receipt<S>(
    stream: &mut S,
    protocol_version: u16,
    remote_locator: &RemoteLocator,
    durable_cursor: u64,
    command: RemoteCommand,
    started: Instant,
) -> Result<RemoteCommandResult, AgentInvokeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let expected_digest = command.digest();
    let expected_command_id = command.command_id();
    write_post_auth(
        stream,
        protocol_version,
        &RemotePostAuth::Command { command },
    )
    .await?;
    let mut command_frames = 0_usize;
    let mut last_transient_sequence = None;
    loop {
        command_frames = command_frames.saturating_add(1);
        if command_frames > MAX_COMMAND_FRAMES || started.elapsed() > EXCHANGE_TIMEOUT {
            return Err(unavailable("remote child command exceeded the frame bound"));
        }
        match read_post_auth(stream, protocol_version).await? {
            RemotePostAuth::CommandResult { result }
                if result.command_id() == expected_command_id
                    && result.digest() == expected_digest =>
            {
                return check_receipt_cursor(result, durable_cursor);
            }
            RemotePostAuth::CommandResult { .. } => {
                return Err(unavailable("remote child received an unrelated receipt"));
            }
            RemotePostAuth::EventBatch { events } => {
                check_live_events(&events, remote_locator, &mut last_transient_sequence)?;
            }
            RemotePostAuth::Grant { .. } => {}
            other => {
                return Err(unavailable(format!(
                    "remote child command returned unexpected {}",
                    other.kind()
                )));
            }
        }
    }
}

fn check_receipt_cursor(
    result: RemoteCommandResult,
    durable_cursor: u64,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    let cursor_valid = if result.accepted() {
        result.durable_sequence() >= durable_cursor
    } else {
        result.durable_sequence() == durable_cursor
    };
    if !cursor_valid {
        return Err(unavailable(
            "remote child received an invalid receipt cursor",
        ));
    }
    Ok(result)
}

fn check_live_events(
    events: &[RemoteEventView],
    remote_locator: &RemoteLocator,
    last_transient_sequence: &mut Option<u64>,
) -> Result<(), AgentInvokeError> {
    for event in events {
        let full = event.event();
        if event.durable_sequence().is_some()
            || full.session_id() != remote_locator.session_id()
            || Some(full.lane_id()) != remote_locator.lane_id()
            || Some(full.run_id()) != remote_locator.run_id()
            || last_transient_sequence.is_some_and(|value| event.transient_sequence() <= value)
        {
            return Err(unavailable("remote child received an invalid live event"));
        }
        *last_transient_sequence = Some(event.transient_sequence());
    }
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    ceiling: usize,
) -> Result<Vec<u8>, AgentInvokeError> {
    let mut header = [0_u8; FRAME_LENGTH_BYTES];
    tokio::time::timeout(READ_TIMEOUT, reader.read_exact(&mut header))
        .await
        .map_err(|_| unavailable("remote child read timed out"))?
        .map_err(|error| unavailable(error.to_string()))?;
    let declared =
        decode_frame_len(header, ceiling).map_err(|error| unavailable(error.to_string()))?;
    let mut payload = vec![0_u8; declared];
    if declared > 0 {
        tokio::time::timeout(READ_TIMEOUT, reader.read_exact(&mut payload))
            .await
            .map_err(|_| unavailable("remote child read timed out"))?
            .map_err(|error| unavailable(error.to_string()))?;
    }
    Ok(payload)
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    ceiling: usize,
) -> Result<(), AgentInvokeError> {
    let frame = encode_frame(payload, ceiling).map_err(|error| unavailable(error.to_string()))?;
    tokio::time::timeout(WRITE_TIMEOUT, writer.write_all(&frame))
        .await
        .map_err(|_| unavailable("remote child write timed out"))?
        .map_err(|error| unavailable(error.to_string()))?;
    tokio::time::timeout(WRITE_TIMEOUT, writer.flush())
        .await
        .map_err(|_| unavailable("remote child flush timed out"))?
        .map_err(|error| unavailable(error.to_string()))
}

async fn read_pre_auth<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<RemotePreAuth, AgentInvokeError> {
    let payload = read_frame(stream, PRE_AUTH_FRAME_MAX_BYTES).await?;
    let envelope: ProtocolEnvelope<RemotePreAuth> =
        decode_envelope(&payload, PayloadFamily::Remote, PROTOCOL_VERSION_V1)
            .map_err(|error| unavailable(error.to_string()))?;
    Ok(envelope.into_body())
}

async fn write_pre_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    body: &RemotePreAuth,
) -> Result<(), AgentInvokeError> {
    let payload = encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body)
        .map_err(|error| unavailable(error.to_string()))?;
    write_frame(stream, &payload, PRE_AUTH_FRAME_MAX_BYTES).await
}

async fn read_post_auth<S: AsyncRead + Unpin>(
    stream: &mut S,
    protocol_version: u16,
) -> Result<RemotePostAuth, AgentInvokeError> {
    let payload = read_frame(stream, POST_AUTH_FRAME_MAX_BYTES).await?;
    let envelope: ProtocolEnvelope<RemotePostAuth> =
        decode_envelope(&payload, PayloadFamily::Remote, protocol_version)
            .map_err(|error| unavailable(error.to_string()))?;
    Ok(envelope.into_body())
}

async fn write_post_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    protocol_version: u16,
    body: &RemotePostAuth,
) -> Result<(), AgentInvokeError> {
    let payload = encode_envelope(PayloadFamily::Remote, protocol_version, body)
        .map_err(|error| unavailable(error.to_string()))?;
    write_frame(stream, &payload, POST_AUTH_FRAME_MAX_BYTES).await
}
