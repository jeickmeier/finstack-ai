//! Length-prefixed PR-058 client over an explicit stream.

use finstack_ai_kernel::ChildRunLocator;
use finstack_ai_protocol::{
    FRAME_LENGTH_BYTES, POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1,
    PayloadFamily, ProtocolEnvelope, RemoteAuthMethod, RemoteCommand, RemoteCommandOp,
    RemoteCommandResult, RemoteLocator, RemotePostAuth, RemotePreAuth, VersionOffer,
    decode_envelope, decode_frame_len, encode_envelope, encode_frame,
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
    op: RemoteCommandOp,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    match parse_endpoint(&route.endpoint)? {
        RemoteEndpoint::Tcp(addr) => {
            let stream =
                tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr))
                    .await
                    .map_err(|_| unavailable("remote child connect timed out"))?
                    .map_err(|error| unavailable(error.to_string()))?;
            exchange_on(stream, route, locator, op, command_id).await
        }
        RemoteEndpoint::Unix(path) => unix_exchange(path, route, locator, op, command_id).await,
    }
}

#[cfg(unix)]
async fn unix_exchange(
    path: std::path::PathBuf,
    route: &RemoteChildRoute,
    locator: &ChildRunLocator,
    op: RemoteCommandOp,
    command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::UnixStream::connect(path))
        .await
        .map_err(|_| unavailable("remote child connect timed out"))?
        .map_err(|error| unavailable(error.to_string()))?;
    exchange_on(stream, route, locator, op, command_id).await
}

#[cfg(not(unix))]
async fn unix_exchange(
    _path: std::path::PathBuf,
    _route: &RemoteChildRoute,
    _locator: &ChildRunLocator,
    _op: RemoteCommandOp,
    _command_id: &str,
) -> Result<RemoteCommandResult, AgentInvokeError> {
    Err(unavailable("unix remote child endpoints are not supported"))
}

pub(crate) async fn exchange_on<S>(
    mut stream: S,
    route: &RemoteChildRoute,
    locator: &ChildRunLocator,
    op: RemoteCommandOp,
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
    let RemotePreAuth::ServerHello { .. } = read_pre_auth(&mut stream).await? else {
        return Err(unavailable("remote child handshake failed"));
    };
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
    let remote_locator = RemoteLocator::new(
        locator.operation.session_id.to_string(),
        Some(locator.operation.lane_id.to_string()),
        Some(locator.operation.run_id.to_string()),
    );
    write_post_auth(
        &mut stream,
        &RemotePostAuth::OpenSession {
            last_known_durable_sequence: None,
            locator: remote_locator.clone(),
            tenant_scope: locator.operation.tenant_scope.to_string(),
        },
    )
    .await?;
    let started = Instant::now();
    let mut open_frames = 0_usize;
    loop {
        open_frames = open_frames.saturating_add(1);
        if open_frames > MAX_OPEN_FRAMES || started.elapsed() > EXCHANGE_TIMEOUT {
            return Err(unavailable("remote child open exceeded the frame bound"));
        }
        match read_post_auth(&mut stream).await? {
            RemotePostAuth::SyncBarrier { .. } => break,
            RemotePostAuth::Snapshot { .. }
            | RemotePostAuth::NoSnapshot { .. }
            | RemotePostAuth::DurableTail { .. } => {}
            RemotePostAuth::EventBatch { live: true, .. } => {
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
    }
    let command = RemoteCommand::try_new(
        command_id,
        remote_locator,
        locator.operation.tenant_scope.as_ref(),
        op,
    )
    .map_err(|error| unavailable(error.to_string()))?;
    write_post_auth(&mut stream, &RemotePostAuth::Command { command }).await?;
    let mut command_frames = 0_usize;
    loop {
        command_frames = command_frames.saturating_add(1);
        if command_frames > MAX_COMMAND_FRAMES || started.elapsed() > EXCHANGE_TIMEOUT {
            return Err(unavailable("remote child command exceeded the frame bound"));
        }
        match read_post_auth(&mut stream).await? {
            RemotePostAuth::CommandResult { result } => return Ok(result),
            RemotePostAuth::EventBatch { .. } | RemotePostAuth::Grant { .. } => {}
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
        decode_envelope(&payload, PayloadFamily::Remote)
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
) -> Result<RemotePostAuth, AgentInvokeError> {
    let payload = read_frame(stream, POST_AUTH_FRAME_MAX_BYTES).await?;
    let envelope: ProtocolEnvelope<RemotePostAuth> =
        decode_envelope(&payload, PayloadFamily::Remote)
            .map_err(|error| unavailable(error.to_string()))?;
    Ok(envelope.into_body())
}

async fn write_post_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    body: &RemotePostAuth,
) -> Result<(), AgentInvokeError> {
    let payload = encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body)
        .map_err(|error| unavailable(error.to_string()))?;
    write_frame(stream, &payload, POST_AUTH_FRAME_MAX_BYTES).await
}
