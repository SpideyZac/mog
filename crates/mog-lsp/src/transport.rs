//! JSON-RPC message framing over a byte stream.
//!
//! Language servers talk with `Content-Length` framed JSON, like HTTP without the rest of HTTP.

use std::io::{self, ErrorKind};

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The header that gives the size of a message body.
const CONTENT_LENGTH: &str = "content-length";

/// A decoded JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A call that expects a [`Message::Response`] with the same id.
    Request {
        /// The id the response will carry.
        id: Value,
        /// The method being called.
        method: String,
        /// The call arguments.
        params: Value,
    },
    /// The answer to a [`Message::Request`].
    Response {
        /// The id of the request being answered.
        id: Value,
        /// The result on success, or the error object on failure.
        result: Result<Value, Value>,
    },
    /// A one way message.
    Notification {
        /// The method being called.
        method: String,
        /// The call arguments.
        params: Value,
    },
}

impl Message {
    /// Decodes a message from its JSON form.
    ///
    /// # Errors
    ///
    /// Returns an error if `value` is not a JSON-RPC message.
    pub fn from_json(value: Value) -> io::Result<Self> {
        let invalid = || io::Error::new(ErrorKind::InvalidData, "not a json-rpc message");
        let mut object = match value {
            Value::Object(object) => object,
            _ => return Err(invalid()),
        };
        let params = object.remove("params").unwrap_or(Value::Null);
        let method = object.remove("method");
        let id = object.remove("id");
        match (id, method) {
            (Some(id), Some(Value::String(method))) => Ok(Self::Request { id, method, params }),
            (None, Some(Value::String(method))) => Ok(Self::Notification { method, params }),
            (Some(id), None) => {
                let result = match object.remove("error") {
                    Some(error) => Err(error),
                    None => Ok(object.remove("result").unwrap_or(Value::Null)),
                };
                Ok(Self::Response { id, result })
            }
            _ => Err(invalid()),
        }
    }

    /// Encodes the message to its JSON form.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Request { id, method, params } => {
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
            }
            Self::Response { id, result } => match result {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
            },
            Self::Notification { method, params } => {
                json!({ "jsonrpc": "2.0", "method": method, "params": params })
            }
        }
    }
}

/// The biggest message a language server may send, so a broken header cannot exhaust memory.
pub const MAX_MESSAGE: usize = 512 << 20;

/// Reads one message, or `None` if the stream ended cleanly before it.
///
/// # Errors
///
/// Returns an error on a broken header, bad JSON, a message over [`MAX_MESSAGE`] or a failed
/// read.
pub async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Message>> {
    read_message_limited(reader, MAX_MESSAGE).await
}

/// Reads one message of at most `max` bytes, or `None` if the stream ended cleanly before it.
///
/// # Errors
///
/// Returns an error on a broken header, bad JSON, a message over `max` or a failed read.
pub async fn read_message_limited<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> io::Result<Option<Message>> {
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
    if length > max {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("a message of {length} bytes is over the limit of {max}"),
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    let value = serde_json::from_slice(&body)?;
    Message::from_json(value).map(Some)
}

/// Writes one message and flushes it.
///
/// # Errors
///
/// Returns an error if the write fails.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Message,
) -> io::Result<()> {
    let body = message.to_json().to_string();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body.as_bytes()).await?;
    writer.flush().await
}

#[cfg(test)]
/// Tests for message framing.
mod tests {
    use serde_json::json;
    use tokio::io::BufReader;

    use super::{Message, read_message, read_message_limited, write_message};

    /// A written message reads back the same.
    #[tokio::test]
    async fn round_trip() {
        let messages = [
            Message::Request {
                id: json!(1),
                method: "initialize".into(),
                params: json!({ "a": 1 }),
            },
            Message::Response {
                id: json!(1),
                result: Err(json!({ "code": -1 })),
            },
            Message::Notification {
                method: "exit".into(),
                params: json!(null),
            },
        ];
        let mut bytes = Vec::new();
        for message in &messages {
            write_message(&mut bytes, message).await.expect("write");
        }
        let mut reader = BufReader::new(bytes.as_slice());
        for message in messages {
            let read = read_message(&mut reader).await.expect("read");
            assert_eq!(read, Some(message));
        }
        assert_eq!(read_message(&mut reader).await.expect("eof"), None);
    }

    /// A message that claims to be bigger than the limit is refused before it is read.
    #[tokio::test]
    async fn refuses_huge_messages() {
        let framed = b"Content-Length: 99999999999\r\n\r\n{}";
        let mut reader = BufReader::new(&framed[..]);
        assert!(read_message_limited(&mut reader, 1024).await.is_err());
    }
}
