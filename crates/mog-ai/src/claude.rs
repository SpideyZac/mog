//! Claude through the Anthropic Messages API, streamed.
//!
//! Claude only chats. Inline suggestions come from Copilot, which is built for them.

use std::time::Duration;

use reqwest::{Client, Response, StatusCode, header::RETRY_AFTER};
use serde_json::{Value, json};
use tokio::time;

use crate::provider::{AiError, AiProvider, BoxFuture, ChatMessage, OnText, Role};

/// The Messages API endpoint.
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// The API version header value.
const API_VERSION: &str = "2023-06-01";

/// The beta that lets the server retry declined requests on another model.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// The model used when the config does not name one.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// The output cap for chat replies. Streaming keeps a long reply from timing out.
const CHAT_MAX_TOKENS: u32 = 64_000;

/// How long connecting to the API may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the API may go quiet mid reply. It sends pings while thinking, so this is generous.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// How many times a request is tried before giving up on a busy or flaky API.
const MAX_ATTEMPTS: u32 = 3;

/// The longest wait between tries, even if the API asks for more.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Talks to Claude.
pub struct Claude {
    /// The HTTP client, reused across requests.
    http: Client,
    /// Where requests go.
    url: String,
    /// The API key sent with each request.
    api_key: String,
    /// The model id.
    model: String,
}

impl Claude {
    /// Creates a provider that uses `model` with `api_key`.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::with_url(MESSAGES_URL, api_key, model)
    }

    /// Creates a provider that sends requests to `url` instead of the Anthropic API.
    pub fn with_url(
        url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            url: url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Posts `body`, trying again with backoff when the API is busy or the network blips.
    async fn post(&self, body: &Value) -> Result<Response, AiError> {
        let mut attempt = 1;
        loop {
            let sent = self
                .http
                .post(&self.url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("anthropic-beta", FALLBACK_BETA)
                .json(body)
                .send()
                .await;
            let wait = match sent {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status();
                    let asked = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.trim().parse::<u64>().ok())
                        .map(Duration::from_secs);
                    let text = response.text().await.unwrap_or_default();
                    if !is_retryable(status) || attempt >= MAX_ATTEMPTS {
                        return Err(AiError::Api(error_message(status, &text)));
                    }
                    asked.unwrap_or_else(|| backoff(attempt))
                }
                Err(err) if (err.is_connect() || err.is_timeout()) && attempt < MAX_ATTEMPTS => {
                    backoff(attempt)
                }
                Err(err) => return Err(err.into()),
            };
            time::sleep(wait.min(MAX_BACKOFF)).await;
            attempt += 1;
        }
    }

    /// Streams the reply to `body`, passing text to `on_text`, and returns all of it.
    async fn stream(&self, body: Value, on_text: OnText<'_>) -> Result<String, AiError> {
        let mut response = self.post(&body).await?;
        let mut parser = SseParser::default();
        let mut reply = Reply::default();
        while let Some(chunk) = response.chunk().await? {
            for event in parser.feed(&chunk) {
                if let Some(text) = reply.handle(&event)? {
                    on_text(&text);
                }
            }
            if reply.done {
                break;
            }
        }
        reply.finish()
    }
}

/// Returns whether a request that got `status` is worth trying again.
fn is_retryable(status: StatusCode) -> bool {
    // 529 is the API saying it is overloaded
    matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error()
}

/// Returns how long to wait before try number `attempt + 1`.
fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500 << attempt.min(6))
}

/// Turns an error response into a message, whether or not its body is JSON.
fn error_message(status: StatusCode, body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| value["error"]["message"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let body = body.trim();
            if body.is_empty() {
                status.canonical_reason().unwrap_or("no details").to_owned()
            } else {
                body.chars().take(200).collect()
            }
        });
    format!("{status}: {message}")
}

/// Builds a streaming Messages API request body.
fn request_body(
    model: &str,
    max_tokens: u32,
    system: Option<&str>,
    messages: &[ChatMessage],
) -> Value {
    let messages: Vec<Value> = messages
        .iter()
        .map(|message| {
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            json!({ "role": role, "content": message.text })
        })
        .collect();
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
        "fallbacks": "default",
    });
    if let Some(system) = system.filter(|system| !system.is_empty()) {
        body["system"] = json!(system);
    }
    body
}

/// Splits a server sent event stream into the JSON of each event.
#[derive(Debug, Default)]
struct SseParser {
    /// Bytes of a line that has not ended yet.
    line: Vec<u8>,
    /// The `data` lines of the event being read.
    data: String,
}

impl SseParser {
    /// Reads `chunk` and returns the events it finished.
    fn feed(&mut self, chunk: &[u8]) -> Vec<Value> {
        let mut events = Vec::new();
        for &byte in chunk {
            if byte != b'\n' {
                self.line.push(byte);
                continue;
            }
            // lines are whole here so a char split across chunks is back together
            let line = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    if let Ok(event) = serde_json::from_str(&self.data) {
                        events.push(event);
                    }
                    self.data.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            }
        }
        events
    }
}

/// A reply being put together from stream events.
#[derive(Debug, Default)]
struct Reply {
    /// The text so far.
    text: String,
    /// Why the model stopped, once it says.
    stop_reason: Option<String>,
    /// Whether the stream said it is over.
    done: bool,
}

impl Reply {
    /// Takes in one event, returning text it added.
    fn handle(&mut self, event: &Value) -> Result<Option<String>, AiError> {
        match event["type"].as_str() {
            Some("content_block_delta") if event["delta"]["type"] == "text_delta" => {
                let text = event["delta"]["text"].as_str().unwrap_or_default();
                self.text.push_str(text);
                return Ok(Some(text.to_owned()).filter(|text| !text.is_empty()));
            }
            Some("message_delta") => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_owned());
                }
            }
            Some("message_stop") => self.done = true,
            Some("error") => {
                let message = event["error"]["message"]
                    .as_str()
                    .unwrap_or("the stream broke off");
                return Err(AiError::Api(message.to_owned()));
            }
            _ => {}
        }
        Ok(None)
    }

    /// Returns the whole reply, or why there is none.
    fn finish(self) -> Result<String, AiError> {
        match self.stop_reason.as_deref() {
            Some("refusal") => Err(AiError::Refused),
            None if !self.done => Err(AiError::Api("the reply was cut off".into())),
            _ => Ok(self.text),
        }
    }
}

impl AiProvider for Claude {
    fn id(&self) -> &str {
        "claude"
    }

    fn chat<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [ChatMessage],
        on_text: OnText<'a>,
    ) -> BoxFuture<'a, Result<String, AiError>> {
        let body = request_body(&self.model, CHAT_MAX_TOKENS, Some(system), messages);
        Box::pin(self.stream(body, on_text))
    }
}

#[cfg(test)]
/// Tests for request building, streaming and retries.
mod tests {
    use std::sync::Mutex;

    use reqwest::StatusCode;
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::{Claude, Reply, SseParser, error_message, is_retryable, request_body};
    use crate::provider::{AiError, AiProvider, ChatMessage, Role};

    /// A user message saying `text`.
    fn user(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            text: text.into(),
        }
    }

    /// Requests carry the model, messages, system prompt, streaming and fallbacks.
    #[test]
    fn builds_request() {
        let body = request_body("claude-opus-5-5", 100, Some("be brief"), &[user("hi")]);
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(
            body["messages"][0],
            json!({ "role": "user", "content": "hi" })
        );
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["stream"], true);
        assert_eq!(body["fallbacks"], "default");
        assert!(request_body("m", 1, Some(""), &[]).get("system").is_none());
    }

    /// Events come out whole even when chunks split lines and chars.
    #[test]
    fn parses_split_events() {
        let stream = "event: ping\ndata: {\"type\":\"ping\"}\n\nevent: content_block_delta\r\n\
                      data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\
                      \"text\":\"h\u{e9}\"}}\r\n\r\n";
        let bytes = stream.as_bytes();
        let mut parser = SseParser::default();
        let mut events = Vec::new();
        for chunk in bytes.chunks(7) {
            events.extend(parser.feed(chunk));
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["delta"]["text"], "h\u{e9}");
    }

    /// Text deltas add up, thinking is skipped and refusals become an error.
    #[test]
    fn builds_replies() {
        let delta = |kind: &str, text: &str| json!({ "type": "content_block_delta", "delta": { "type": kind, "text": text } });
        let mut reply = Reply::default();
        assert_eq!(
            reply.handle(&delta("text_delta", "a")).expect("ok"),
            Some("a".into())
        );
        assert_eq!(
            reply.handle(&delta("thinking_delta", "x")).expect("ok"),
            None
        );
        reply.handle(&delta("text_delta", "b")).expect("ok");
        reply
            .handle(&json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }))
            .expect("ok");
        reply
            .handle(&json!({ "type": "message_stop" }))
            .expect("ok");
        assert_eq!(reply.finish().expect("text"), "ab");
        let mut refused = Reply::default();
        refused
            .handle(&json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" } }))
            .expect("ok");
        assert!(matches!(refused.finish(), Err(AiError::Refused)));
        let error = json!({ "type": "error", "error": { "message": "overloaded" } });
        assert!(Reply::default().handle(&error).is_err());
        assert!(Reply::default().finish().is_err());
    }

    /// Error bodies that are not JSON still give a readable message.
    #[test]
    fn reads_error_bodies() {
        let json = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#;
        assert_eq!(
            error_message(StatusCode::BAD_REQUEST, json),
            "400 Bad Request: bad"
        );
        assert_eq!(
            error_message(StatusCode::BAD_GATEWAY, "<html>oops</html>"),
            "502 Bad Gateway: <html>oops</html>"
        );
        assert_eq!(
            error_message(StatusCode::FORBIDDEN, ""),
            "403 Forbidden: Forbidden"
        );
    }

    /// Busy and broken servers are retried, bad requests are not.
    #[test]
    fn picks_retries() {
        assert!(is_retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable(StatusCode::from_u16(529).expect("status")));
        assert!(is_retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!is_retryable(StatusCode::BAD_REQUEST));
        assert!(!is_retryable(StatusCode::UNAUTHORIZED));
    }

    /// An overloaded answer is retried and the streamed reply after it comes through.
    #[tokio::test]
    async fn retries_then_streams() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!(
            "http://{}/v1/messages",
            listener.local_addr().expect("addr")
        );
        let events = [
            json!({ "type": "message_start", "message": {} }),
            json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "hel" } }),
            json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "lo" } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }),
            json!({ "type": "message_stop" }),
        ];
        let stream: String = events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap_or("")
                )
            })
            .collect();
        tokio::spawn(async move {
            let busy = "{\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}";
            let answers = [
                format!(
                    "HTTP/1.1 529 Overloaded\r\nretry-after: 0\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{busy}",
                    busy.len()
                ),
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{stream}",
                    stream.len()
                ),
            ];
            for answer in answers {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut request = vec![0; 1 << 16];
                let _ = socket.read(&mut request).await;
                let _ = socket.write_all(answer.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        let claude = Claude::with_url(url, "key", "model");
        let seen = Mutex::new(String::new());
        let on_text = |text: &str| seen.lock().expect("lock").push_str(text);
        let reply = claude
            .chat("system", &[user("hi")], &on_text)
            .await
            .expect("reply");
        assert_eq!(reply, "hello");
        assert_eq!(*seen.lock().expect("lock"), "hello");
    }
}
