//! Claude through the Anthropic Messages API.

use reqwest::Client;
use serde_json::{Value, json};

use crate::provider::{AiError, AiProvider, BoxFuture, ChatMessage, CompletionRequest, Role};

/// The Messages API endpoint.
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// The API version header value.
const API_VERSION: &str = "2023-06-01";

/// The beta that lets the server retry declined requests on another model.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// The model used when the config does not name one.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// The output cap for chat replies.
const CHAT_MAX_TOKENS: u32 = 16_000;

/// The output cap for inline completions, which should be short.
const COMPLETION_MAX_TOKENS: u32 = 1_024;

/// The instructions for inline completions.
const COMPLETION_SYSTEM: &str = "You complete code in a text editor. Reply with only the text \
that belongs at the cursor, with no explanation and no code fences. Reply with nothing if no \
completion makes sense.";

/// The marker placed at the cursor in completion prompts.
const CURSOR: &str = "<cursor/>";

/// Talks to Claude.
pub struct Claude {
    /// The HTTP client, reused across requests.
    http: Client,
    /// The API key sent with each request.
    api_key: String,
    /// The model id.
    model: String,
}

impl Claude {
    /// Creates a provider that uses `model` with `api_key`.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Sends a request body and returns the reply text.
    async fn send(&self, body: Value) -> Result<String, AiError> {
        let response = self
            .http
            .post(MESSAGES_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success() {
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(AiError::Api(format!("{status}: {message}")));
        }
        reply_text(&value)
    }
}

/// Builds a Messages API request body.
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
        "fallbacks": "default",
    });
    if let Some(system) = system {
        body["system"] = json!(system);
    }
    body
}

/// Builds the user prompt for an inline completion.
fn completion_prompt(request: &CompletionRequest) -> String {
    let language = request.language.as_deref().unwrap_or("unknown");
    format!(
        "Language: {language}\n\n{}{CURSOR}{}",
        request.prefix, request.suffix
    )
}

/// Pulls the reply text out of a Messages API response.
fn reply_text(response: &Value) -> Result<String, AiError> {
    if response["stop_reason"] == "refusal" {
        return Err(AiError::Refused);
    }
    let text = response["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default();
    Ok(text)
}

impl AiProvider for Claude {
    fn id(&self) -> &str {
        "claude"
    }

    fn chat<'a>(&'a self, messages: &'a [ChatMessage]) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(self.send(request_body(&self.model, CHAT_MAX_TOKENS, None, messages)))
    }

    fn complete<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> BoxFuture<'a, Result<Option<String>, AiError>> {
        Box::pin(async move {
            let prompt = ChatMessage {
                role: Role::User,
                text: completion_prompt(request),
            };
            let body = request_body(
                &self.model,
                COMPLETION_MAX_TOKENS,
                Some(COMPLETION_SYSTEM),
                &[prompt],
            );
            let text = self.send(body).await?;
            Ok(Some(text).filter(|text| !text.trim().is_empty()))
        })
    }
}

#[cfg(test)]
/// Tests for request building and response parsing.
mod tests {
    use serde_json::json;

    use super::{reply_text, request_body};
    use crate::provider::{AiError, ChatMessage, Role};

    /// Requests carry the model, messages, system prompt and fallbacks.
    #[test]
    fn builds_request() {
        let messages = [ChatMessage {
            role: Role::User,
            text: "hi".into(),
        }];
        let body = request_body("claude-opus-5-5", 100, Some("be brief"), &messages);
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(
            body["messages"][0],
            json!({ "role": "user", "content": "hi" })
        );
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["fallbacks"], "default");
    }

    /// Text blocks are joined and other blocks are skipped.
    #[test]
    fn joins_text_blocks() {
        let response = json!({
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "a" },
                { "type": "text", "text": "b" },
            ],
        });
        assert_eq!(reply_text(&response).expect("text"), "ab");
    }

    /// Refusals become an error instead of empty text.
    #[test]
    fn refusal_is_an_error() {
        let response = json!({ "stop_reason": "refusal", "content": [] });
        assert!(matches!(reply_text(&response), Err(AiError::Refused)));
    }
}
