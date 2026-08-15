//! Length-prefixed async frame I/O.

use finstack_ai_protocol::{FRAME_LENGTH_BYTES, decode_frame_len, encode_frame};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ServerError;

/// Read one frame, rejecting an oversize length before allocating the payload.
///
/// # Errors
///
/// Returns protocol, I/O, or limit failures.
pub async fn read_frame<R: AsyncRead + Unpin>(
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
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    ceiling: usize,
) -> Result<(), ServerError> {
    let frame = encode_frame(payload, ceiling)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}
