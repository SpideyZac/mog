//! `Content-Length` framed JSON over a byte stream.

use std::io::{self, ErrorKind};

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The header that gives the size of a message body.
const CONTENT_LENGTH: &str = "content-length";

/// Reads one message, or `None` if the stream ended cleanly before it.
///
/// # Errors
///
/// Returns an error on a broken header, bad JSON or a failed read.
pub async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return if length.is_none() {
                Ok(None)
            } else {
                Err(ErrorKind::UnexpectedEof.into())
            };
        }
        let header = line.trim_end();
        if header.is_empty() {
            // some adapters print a stray blank line before the first header
            if length.is_none() {
                continue;
            }
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case(CONTENT_LENGTH)
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length =
        length.ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "missing content length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

/// Writes `message` with its header.
///
/// # Errors
///
/// Returns an error if writing fails.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Value,
) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(&body).await?;
    writer.flush().await
}

#[cfg(test)]
/// Tests for framing.
mod tests {
    use serde_json::json;
    use tokio::io::BufReader;

    use super::{read_message, write_message};

    /// A written message reads back the same.
    #[tokio::test]
    async fn round_trips() {
        let mut bytes = Vec::new();
        let message = json!({ "seq": 1, "type": "request", "command": "threads" });
        write_message(&mut bytes, &message).await.expect("write");
        let mut reader = BufReader::new(&bytes[..]);
        assert_eq!(
            read_message(&mut reader).await.expect("read"),
            Some(message)
        );
        assert_eq!(read_message(&mut reader).await.expect("read"), None);
    }
}
