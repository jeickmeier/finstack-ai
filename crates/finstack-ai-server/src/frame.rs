//! Length-prefixed async frame I/O.

use finstack_ai_protocol::{
    FRAME_LENGTH_BYTES, PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1, PayloadFamily,
    ProtocolEnvelope, RemotePostAuth, RemotePreAuth, decode_envelope, decode_frame_len,
    encode_envelope, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ServerError;

/// Read one frame, rejecting an oversize length before allocating the payload.
///
/// # Errors
///
/// Returns protocol, I/O, or limit failures.
pub(crate) async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    ceiling: usize,
) -> Result<Vec<u8>, ServerError> {
    let mut header = [0_u8; FRAME_LENGTH_BYTES];
    reader.read_exact(&mut header).await?;
    let declared = decode_frame_len(header, ceiling)?;
    let mut payload = vec![0_u8; declared];
    if declared > 0 {
        reader.read_exact(&mut payload).await?;
    }
    Ok(payload)
}

/// Write one length-prefixed frame.
///
/// # Errors
///
/// Returns protocol or I/O failures.
pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    ceiling: usize,
) -> Result<(), ServerError> {
    let frame = encode_frame(payload, ceiling)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one remote pre-auth envelope.
///
/// # Errors
///
/// Returns protocol, I/O, or limit failures.
pub(crate) async fn read_pre_auth<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<RemotePreAuth, ServerError> {
    let payload = read_frame(stream, PRE_AUTH_FRAME_MAX_BYTES).await?;
    let envelope: ProtocolEnvelope<RemotePreAuth> =
        decode_envelope(&payload, PayloadFamily::Remote)?;
    Ok(envelope.into_body())
}

/// Write one remote pre-auth envelope.
///
/// # Errors
///
/// Returns protocol or I/O failures.
pub(crate) async fn write_pre_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    body: &RemotePreAuth,
) -> Result<(), ServerError> {
    let payload = encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body)?;
    write_frame(stream, &payload, PRE_AUTH_FRAME_MAX_BYTES).await
}

/// Read one remote post-auth envelope.
///
/// # Errors
///
/// Returns protocol, I/O, or limit failures.
pub(crate) async fn read_post_auth<S: AsyncRead + Unpin>(
    stream: &mut S,
    ceiling: usize,
) -> Result<RemotePostAuth, ServerError> {
    let payload = read_frame(stream, ceiling).await?;
    let envelope: ProtocolEnvelope<RemotePostAuth> =
        decode_envelope(&payload, PayloadFamily::Remote)?;
    Ok(envelope.into_body())
}

/// Write one remote post-auth envelope.
///
/// # Errors
///
/// Returns protocol or I/O failures.
pub(crate) async fn write_post_auth<S: AsyncWrite + Unpin>(
    stream: &mut S,
    ceiling: usize,
    body: &RemotePostAuth,
) -> Result<(), ServerError> {
    let payload = encode_envelope(PayloadFamily::Remote, PROTOCOL_VERSION_V1, body)?;
    write_frame(stream, &payload, ceiling).await
}
