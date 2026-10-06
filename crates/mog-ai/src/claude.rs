//! Claude through the Anthropic Messages API, streamed.
//!
//! Claude only chats. Inline suggestions come from Copilot, which is built for them.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use reqwest::{Client, Response, StatusCode, header::RETRY_AFTER};
use serde_json::{Value, json};
use tokio::time;

use crate::provider::{AiError, AiProvider, BoxFuture, CallTool, ChatMessage, OnText, Role, Tool};

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

/// How many times in one answer the model may call tools before mog stops it.
const MAX_TOOL_ROUNDS: usize = 16;

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
    /// Whether requests ask for server side fallbacks, turned off if the API stops knowing them.
    fallbacks: AtomicBool,
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
            fallbacks: AtomicBool::new(true),
        }
    }

    /// Posts `body`, trying again with backoff when the API is busy or the network blips.
    async fn post(&self, body: &Value) -> Result<Response, AiError> {
        let mut attempt = 1;
        loop {
            let fallbacks = self.fallbacks.load(Ordering::Relaxed);
            let mut request = self
                .http
                .post(&self.url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION);
            let sent = if fallbacks {
                request = request.header("anthropic-beta", FALLBACK_BETA);
                request.json(body).send().await
            } else {
                let mut body = body.clone();
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("fallbacks");
                }
                request.json(&body).send().await
            };
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
                    // fallbacks are a beta, so if the api stops knowing them chat goes on without
                    if fallbacks && status == StatusCode::BAD_REQUEST && mentions_fallbacks(&text) {
                        self.fallbacks.store(false, Ordering::Relaxed);
                        continue;
                    }
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
        self.stream_reply(body, on_text).await?.finish()
    }

    /// Streams the reply to `body`, passing text to `on_text`, and returns it once it ends.
    async fn stream_reply(&self, body: Value, on_text: OnText<'_>) -> Result<Reply, AiError> {
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
        Ok(reply)
    }

    /// Chats with `tools`, running the ones the model calls and sending back what they gave,
    /// until it answers.
    async fn chat_tools(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[Tool],
        call: CallTool<'_>,
        on_text: OnText<'_>,
    ) -> Result<String, AiError> {
        let mut body = request_body(&self.model, CHAT_MAX_TOKENS, Some(system), messages);
        body["tools"] = tools_json(tools);
        let mut text = String::new();
        for _ in 0..MAX_TOOL_ROUNDS {
            let reply = self.stream_reply(body.clone(), on_text).await?;
            text.push_str(&reply.text);
            match reply.stop_reason.as_deref() {
                Some("tool_use") => {}
                // a refusal or a cut off can leave a tool call half written, so none run
                Some("refusal") => return Err(AiError::Refused),
                Some("max_tokens") if reply.calls().next().is_some() => {
                    return Err(AiError::Api("the reply was cut off in a tool call".into()));
                }
                _ => return reply.finish().map(|_| text),
            }
            let mut results = Vec::new();
            for (id, name, input) in reply.calls() {
                let note = format!("\n\n[{name}]\n\n");
                on_text(&note);
                text.push_str(&note);
                let result = match (reply.invalid.get(id), input.is_object()) {
                    (Some(raw), _) => Err(json!({ "INVALID_JSON": raw }).to_string()),
                    (None, false) => Err("the input must be a JSON object".to_owned()),
                    (None, true) => call(name.to_owned(), input.clone()).await,
                };
                let (content, is_error) = match result {
                    Ok(content) => (content, false),
                    Err(error) => (error, true),
                };
                results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": content,
                    "is_error": is_error,
                }));
            }
            // the whole turn goes back as it came, thinking blocks too, so it stays valid
            if let Some(messages) = body["messages"].as_array_mut() {
                messages.push(json!({ "role": "assistant", "content": reply.blocks }));
                messages.push(json!({ "role": "user", "content": results }));
            }
        }
        Err(AiError::Api(format!(
            "the model called tools {MAX_TOOL_ROUNDS} times without answering"
        )))
    }
}

/// Returns whether a request that got `status` is worth trying again.
fn is_retryable(status: StatusCode) -> bool {
    // 529 is the API saying it is overloaded
    matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error()
}

/// Returns whether an error body complains about the fallback beta or its field.
fn mentions_fallbacks(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("fallback") || body.contains(FALLBACK_BETA)
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

/// Returns `tools` as the Messages API wants them, streaming their input as it is written.
fn tools_json(tools: &[Tool]) -> Value {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
                "eager_input_streaming": true,
            })
        })
        .collect()
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
    /// Every content block as it should be sent back, by index.
    blocks: Vec<Value>,
    /// The input of each tool call being written, as JSON text, by block index.
    inputs: HashMap<usize, String>,
    /// Tool call inputs that were not valid JSON, by tool use id.
    invalid: HashMap<String, String>,
    /// Why the model stopped, once it says.
    stop_reason: Option<String>,
    /// Whether the stream said it is over.
    done: bool,
}

impl Reply {
    /// Returns the tool calls in the reply as `(id, name, input)`.
    fn calls(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.blocks.iter().filter_map(|block| {
            (block["type"] == "tool_use").then_some(())?;
            Some((
                block["id"].as_str()?,
                block["name"].as_str()?,
                &block["input"],
            ))
        })
    }

    /// Returns the block at the index `event` names, if it started.
    fn block(&mut self, event: &Value) -> Option<&mut Value> {
        let index = usize::try_from(event["index"].as_u64()?).ok()?;
        self.blocks.get_mut(index)
    }

    /// Appends `text` to the string field `key` of the block `event` is about.
    fn append(&mut self, event: &Value, key: &str, text: &str) {
        if let Some(block) = self.block(event) {
            let mut joined = block[key].as_str().unwrap_or_default().to_owned();
            joined.push_str(text);
            block[key] = json!(joined);
        }
    }

    /// Takes in one event, returning text it added.
    fn handle(&mut self, event: &Value) -> Result<Option<String>, AiError> {
        let delta = &event["delta"];
        match event["type"].as_str() {
            Some("content_block_start") => {
                let index = event["index"]
                    .as_u64()
                    .and_then(|index| usize::try_from(index).ok())
                    .unwrap_or(self.blocks.len());
                let mut block = event["content_block"].clone();
                if block["type"] == "tool_use" {
                    self.inputs.insert(index, String::new());
                    block["input"] = json!({});
                }
                if self.blocks.len() <= index {
                    self.blocks.resize(index + 1, Value::Null);
                }
                self.blocks[index] = block;
            }
            Some("content_block_delta") if delta["type"] == "text_delta" => {
                let text = delta["text"].as_str().unwrap_or_default();
                self.text.push_str(text);
                self.append(event, "text", text);
                return Ok(Some(text.to_owned()).filter(|text| !text.is_empty()));
            }
            Some("content_block_delta") if delta["type"] == "thinking_delta" => {
                self.append(
                    event,
                    "thinking",
                    delta["thinking"].as_str().unwrap_or_default(),
                );
            }
            Some("content_block_delta") if delta["type"] == "signature_delta" => {
                if let Some(block) = self.block(event) {
                    block["signature"] = delta["signature"].clone();
                }
            }
            Some("content_block_delta") if delta["type"] == "input_json_delta" => {
                let index = event["index"]
                    .as_u64()
                    .and_then(|index| usize::try_from(index).ok());
                if let Some(input) = index.and_then(|index| self.inputs.get_mut(&index)) {
                    input.push_str(delta["partial_json"].as_str().unwrap_or_default());
                }
            }
            Some("content_block_stop") => {
                let index = event["index"]
                    .as_u64()
                    .and_then(|index| usize::try_from(index).ok());
                let Some((index, raw)) =
                    index.and_then(|index| Some((index, self.inputs.remove(&index)?)))
                else {
                    return Ok(None);
                };
                // inputs stream as written, so they are checked strictly here
                let parsed = if raw.trim().is_empty() {
                    Ok(json!({}))
                } else {
                    serde_json::from_str::<Value>(&raw)
                };
                if let Some(block) = self.blocks.get_mut(index) {
                    match parsed {
                        Ok(input) => block["input"] = input,
                        Err(_) => {
                            let id = block["id"].as_str().unwrap_or_default().to_owned();
                            self.invalid.insert(id, raw);
                        }
                    }
                }
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

    fn chat_with_tools<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [ChatMessage],
        tools: &'a [Tool],
        call: CallTool<'a>,
        on_text: OnText<'a>,
    ) -> BoxFuture<'a, Result<String, AiError>> {
        if tools.is_empty() {
            return self.chat(system, messages, on_text);
        }
        Box::pin(self.chat_tools(system, messages, tools, call, on_text))
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
        net::{TcpListener, TcpStream},
    };

    use super::{Claude, Reply, SseParser, error_message, is_retryable, request_body};
    use crate::provider::{AiError, AiProvider, BoxFuture, ChatMessage, Role, Tool};

    /// Returns `events` as a streamed HTTP answer.
    fn sse_answer(events: &[serde_json::Value]) -> String {
        let stream: String = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{stream}",
            stream.len()
        )
    }

    /// Reads one whole HTTP request from `socket` and returns its body.
    async fn read_body(socket: &mut TcpStream) -> String {
        let mut seen = Vec::new();
        let mut buffer = vec![0; 1 << 16];
        loop {
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            seen.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&seen).into_owned();
            let Some((head, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if body.len() >= length {
                return body.to_owned();
            }
        }
        String::new()
    }

    /// A tool call runs, its result goes back with the whole turn, and the answer after it
    /// comes through.
    #[tokio::test]
    async fn runs_tools() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!(
            "http://{}/v1/messages",
            listener.local_addr().expect("addr")
        );
        let start = |index: u64, block: serde_json::Value| json!({ "type": "content_block_start", "index": index, "content_block": block });
        let delta = |index: u64, delta: serde_json::Value| json!({ "type": "content_block_delta", "index": index, "delta": delta });
        let stop = |index: u64| json!({ "type": "content_block_stop", "index": index });
        let first = [
            start(0, json!({ "type": "thinking", "thinking": "" })),
            delta(0, json!({ "type": "thinking_delta", "thinking": "" })),
            delta(0, json!({ "type": "signature_delta", "signature": "sig" })),
            stop(0),
            start(1, json!({ "type": "text", "text": "" })),
            delta(1, json!({ "type": "text_delta", "text": "Looking." })),
            stop(1),
            start(
                2,
                json!({ "type": "tool_use", "id": "toolu_1", "name": "count", "input": {} }),
            ),
            delta(
                2,
                json!({ "type": "input_json_delta", "partial_json": "{\"pa" }),
            ),
            delta(
                2,
                json!({ "type": "input_json_delta", "partial_json": "th\": \"a.md\"}" }),
            ),
            stop(2),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" } }),
            json!({ "type": "message_stop" }),
        ];
        let second = [
            start(0, json!({ "type": "text", "text": "" })),
            delta(0, json!({ "type": "text_delta", "text": "Two." })),
            stop(0),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }),
            json!({ "type": "message_stop" }),
        ];
        let server = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for answer in [sse_answer(&first), sse_answer(&second)] {
                let (mut socket, _) = listener.accept().await.expect("accept");
                bodies.push(read_body(&mut socket).await);
                let _ = socket.write_all(answer.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            bodies
        });
        let claude = Claude::with_url(url, "key", "model");
        let tools = [Tool {
            name: "count".into(),
            description: "Counts notes".into(),
            input_schema: json!({ "type": "object" }),
        }];
        let call = |name: String,
                    input: serde_json::Value|
         -> BoxFuture<'static, Result<String, String>> {
            Box::pin(async move {
                Ok(format!(
                    "{name} {} = 2",
                    input["path"].as_str().unwrap_or("?")
                ))
            })
        };
        let reply = claude
            .chat_with_tools("system", &[user("how many?")], &tools, &call, &|_: &str| {})
            .await
            .expect("reply");
        assert_eq!(reply, "Looking.\n\n[count]\n\nTwo.");
        let bodies = server.await.expect("server");
        let first: serde_json::Value = serde_json::from_str(&bodies[0]).expect("json");
        assert_eq!(first["tools"][0]["eager_input_streaming"], true);
        let second: serde_json::Value = serde_json::from_str(&bodies[1]).expect("json");
        let messages = second["messages"].as_array().expect("messages");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["signature"], "sig");
        assert_eq!(messages[1]["content"][2]["input"]["path"], "a.md");
        assert_eq!(
            messages[2]["content"][0],
            json!({ "type": "tool_result", "tool_use_id": "toolu_1", "content": "count a.md = 2", "is_error": false })
        );
    }

    /// A tool input that is not valid JSON goes back as an error instead of running the tool.
    #[test]
    fn spots_broken_tool_input() {
        let mut reply = Reply::default();
        let events = [
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "t", "name": "x", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"a\": " } }),
            json!({ "type": "content_block_stop", "index": 0 }),
        ];
        for event in &events {
            reply.handle(event).expect("ok");
        }
        assert_eq!(reply.invalid.get("t").map(String::as_str), Some("{\"a\": "));
    }

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

    /// An API that no longer knows the fallback beta gets the request again without it.
    #[tokio::test]
    async fn drops_unknown_fallbacks() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!(
            "http://{}/v1/messages",
            listener.local_addr().expect("addr")
        );
        let stop = json!({ "type": "message_stop" });
        let delta = json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "ok" } });
        let stream = format!("data: {delta}\n\ndata: {stop}\n\n");
        tokio::spawn(async move {
            let rejected = "{\"type\":\"error\",\"error\":{\"message\":\"fallbacks: Extra inputs are not permitted\"}}";
            let answers = [
                format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{rejected}",
                    rejected.len()
                ),
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{stream}",
                    stream.len()
                ),
            ];
            for (index, answer) in answers.into_iter().enumerate() {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut request = vec![0; 1 << 16];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_lowercase();
                // the retry leaves out both the header and the field
                assert_eq!(request.contains("fallback"), index == 0, "{request}");
                let _ = socket.write_all(answer.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        let claude = Claude::with_url(url, "key", "model");
        let reply = claude
            .chat("system", &[user("hi")], &|_: &str| {})
            .await
            .expect("reply");
        assert_eq!(reply, "ok");
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
