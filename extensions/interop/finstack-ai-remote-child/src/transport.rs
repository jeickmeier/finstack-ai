//! Length-prefixed PR-058 client over an explicit stream.

use finstack_ai_kernel::ChildRunLocator;
use finstack_ai_protocol::{
    FRAME_LENGTH_BYTES, POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1,
    PayloadFamily, ProtocolEnvelope, RemoteAuthMethod, RemoteCommand, RemoteCommandPayload,
    RemoteCommandResult, RemoteLocator, RemotePostAuth, RemotePreAuth, VersionOffer,
    decode_envelope, decode_frame_len, encode_envelope, encode_frame, select_version,
};
use finstack_ai_runtime::AgentInvokeError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Duration, Instant};

use crate::route::{RemoteChildRoute, RemoteEndpoint, parse_endpoint, unavailable};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OPEN_FRAMES: usize = 64;
const MAX_COMMAND_FRAMES: usize = 64;

pub(crate) async fn exchange(
    route: &RemoteChildRoute,
    locator: &ChildRunLocator,
    payload: RemoteCommandPayload,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    match parse_endpoint(&route.endpoint)? {
        RemoteEndpoint::Tcp(addr) => {
            let stream =
                tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr))
                    .await
                    .map_err(|_| unavailable("remote child connect timed out"))?
                    .map_err(|error| unavailable(error.to_string()))?;
            exchange_on(stream, route, locator, payload, command_id).await
        }
        RemoteEndpoint::Unix(path) => {
            unix_exchange(path, route, locator, payload, command_id).await
        }
    }
}

#[cfg(unix)]
async fn unix_exchange(
    path: std::path::PathBuf,
    route: &RemoteChildRoute,
    locator: &ChildRunLocator,
    payload: RemoteCommandPayload,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::UnixStream::connect(path))
        .await
        .map_err(|_| unavailable("remote child connect timed out"))?
        .map_err(|error| unavailable(error.to_string()))?;
    exchange_on(stream, route, locator, payload, command_id).await
}

#[cfg(not(unix))]
async fn unix_exchange(
    _path: std::path::PathBuf,
    _route: &RemoteChildRoute,
    _locator: &ChildRunLocator,
    _payload: RemoteCommandPayload,
    _command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    Err(unavailable("unix remote child endpoints are not supported"))
}

#[expect(
    clippy::too_many_lines,
    reason = "the bounded handshake, reconnect, and receipt state machine is kept linear for auditability"
)]
pub(crate) async fn exchange_on<S>(
    mut stream: S,
    route: &RemoteChildRoute,
    locator: &ChildRunLocator,
    payload: RemoteCommandPayload,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError>
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
        &mut stream,
        &RemotePreAuth::ClientHello {
            offer: offer.clone(),
        },
    )
    .await?;
    let RemotePreAuth::ServerHello {
        selected_version,
        offer: server_offer,
    } = read_pre_auth(&mut stream).await?
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
    let method = match route.token.as_deref() {
        Some(token) if !token.is_empty() => RemoteAuthMethod::Bearer {
            token: token.to_owned(),
        },
        _ => RemoteAuthMethod::Loopback,
    };
    write_pre_auth(&mut stream, &RemotePreAuth::Authenticate { method }).await?;
    let RemotePreAuth::AuthResult { accepted: true, .. } = read_pre_auth(&mut stream).await? else {
        return Err(unavailable("remote child authentication failed"));
    };
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
    let mut open_frames = 0_usize;
    let mut saw_snapshot_choice = false;
    let mut durable_cursor = 0_u64;
    let durable_cursor = loop {
        open_frames = open_frames.saturating_add(1);
        if open_frames > MAX_OPEN_FRAMES || started.elapsed() > EXCHANGE_TIMEOUT {
            return Err(unavailable("remote child open exceeded the frame bound"));
        }
        match read_post_auth(&mut stream, selected_version).await? {
            RemotePostAuth::Snapshot { sequence, snapshot } => {
                if saw_snapshot_choice
                    || snapshot.sequence() != sequence
                    || snapshot.locator() != &remote_locator
                {
                    return Err(unavailable("remote child received an invalid snapshot"));
                }
                saw_snapshot_choice = true;
                durable_cursor = sequence;
            }
            RemotePostAuth::NoSnapshot { sequence } => {
                if saw_snapshot_choice {
                    return Err(unavailable(
                        "remote child received duplicate snapshot state",
                    ));
                }
                saw_snapshot_choice = true;
                durable_cursor = sequence;
            }
            RemotePostAuth::DurableTail {
                from_sequence,
                to_sequence,
                steps,
            } => {
                if !saw_snapshot_choice || from_sequence != durable_cursor.saturating_add(1) {
                    return Err(unavailable("remote child received a discontinuous tail"));
                }
                for step in steps {
                    durable_cursor = durable_cursor.saturating_add(1);
                    if step.sequence() != durable_cursor {
                        return Err(unavailable("remote child received a tail gap"));
                    }
                }
                if durable_cursor != to_sequence {
                    return Err(unavailable("remote child received an invalid tail range"));
                }
            }
            RemotePostAuth::SyncBarrier { sequence } => {
                if !saw_snapshot_choice || sequence != durable_cursor {
                    return Err(unavailable("remote child received the wrong sync barrier"));
                }
                break sequence;
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
    };
    let command = RemoteCommand::try_new(
        command_id,
        remote_locator.clone(),
        locator.operation.tenant_scope.as_ref(),
        durable_cursor,
        payload,
    )
    .map_err(|error| unavailable(error.to_string()))?;
    let expected_digest = command.digest();
    let expected_command_id = command.command_id();
    write_post_auth(
        &mut stream,
        selected_version,
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
        match read_post_auth(&mut stream, selected_version).await? {
            RemotePostAuth::CommandResult { result }
                if result.command_id() == expected_command_id
                    && result.digest() == expected_digest =>
            {
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
                return Ok(result);
            }
            RemotePostAuth::CommandResult { .. } => {
                return Err(unavailable("remote child received an unrelated receipt"));
            }
            RemotePostAuth::EventBatch { events } => {
                for event in events {
                    let full = event.event();
                    if event.durable_sequence().is_some()
                        || full.session_id() != remote_locator.session_id()
                        || Some(full.lane_id()) != remote_locator.lane_id()
                        || Some(full.run_id()) != remote_locator.run_id()
                        || last_transient_sequence
                            .is_some_and(|value| event.transient_sequence() <= value)
                    {
                        return Err(unavailable("remote child received an invalid live event"));
                    }
                    last_transient_sequence = Some(event.transient_sequence());
                }
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
    writer
        .write_all(&frame)
        .await
        .map_err(|error| unavailable(error.to_string()))?;
    writer
        .flush()
        .await
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
