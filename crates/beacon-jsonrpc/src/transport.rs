//! Framed length-prefixed transport helpers.
//!
//! Enabled with the `transport` feature flag.
//!
//! Each message is prefixed with a 4-byte big-endian length (bytes), followed by the UTF-8
//! JSON body. This matches the framing used by the beacon plugin-host gRPC streams and is
//! intended for future hostd↔client and Arc WebSocket migrations.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::{Request, Response, RpcError};

/// Write a single [`Request`] as a length-prefixed frame to `writer`.
pub async fn write_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    req: &Request,
) -> Result<(), RpcError> {
    let json = serde_json::to_vec(req).map_err(|e| RpcError::internal(e.to_string()))?;
    send_frame(writer, json).await
}

/// Write a single [`Response`] as a length-prefixed frame to `writer`.
pub async fn write_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    resp: &Response,
) -> Result<(), RpcError> {
    let json = serde_json::to_vec(resp).map_err(|e| RpcError::internal(e.to_string()))?;
    send_frame(writer, json).await
}

/// Read one [`Request`] from a length-prefixed frame on `reader`.
pub async fn read_request<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Request, RpcError> {
    let frame = recv_frame(reader).await?;
    serde_json::from_slice(&frame).map_err(|e| RpcError::parse_error(e.to_string()))
}

/// Read one [`Response`] from a length-prefixed frame on `reader`.
pub async fn read_response<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Response, RpcError> {
    let frame = recv_frame(reader).await?;
    serde_json::from_slice(&frame).map_err(|e| RpcError::parse_error(e.to_string()))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, payload: Vec<u8>) -> Result<(), RpcError> {
    use tokio::io::AsyncWriteExt;
    let len = payload.len() as u32;
    writer
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    writer.flush().await.map_err(|e| RpcError::internal(e.to_string()))
}

async fn recv_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, RpcError> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| RpcError::internal(format!("transport read len: {e}")))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| RpcError::internal(format!("transport read body: {e}")))?;
    Ok(body)
}
