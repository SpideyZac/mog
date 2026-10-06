//! The [`AiProvider`] trait every AI backend implements.

use std::{future::Future, path::PathBuf, pin::Pin};

use ropey::Rope;
use serde_json::Value;
use thiserror::Error;

/// A boxed future, used so [`AiProvider`] works as a trait object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An error from an AI provider.
#[derive(Debug, Error)]
pub enum AiError {
    /// The provider is missing something it needs, like an API key.
    #[error("{0} is not configured")]
    NotConfigured(String),
    /// The provider does not support this kind of request yet.
    #[error("{0} does not support this yet")]
    Unsupported(String),
    /// The request could not be sent or the response could not be read.
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The provider answered with an error.
    #[error("provider error: {0}")]
    Api(String),
    /// The model declined to answer.
    #[error("the model declined to answer")]
    Refused,
}

/// Who wrote a [`ChatMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The person using the editor.
    User,
    /// The model.
    Assistant,
}

/// One turn of a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Who wrote the message.
    pub role: Role,
    /// The message text.
    pub text: String,
}

/// A request to fill in code at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    /// The text before the cursor.
    pub prefix: String,
    /// The text after the cursor.
    pub suffix: String,
    /// The language of the file, if known.
    pub language: Option<String>,
    /// The whole file, for providers that keep their own copy of it.
    pub file: Option<CompletionFile>,
    /// Whether the user asked for suggestions instead of just pausing, which can give more.
    pub invoked: bool,
}

/// The whole file a [`CompletionRequest`] is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionFile {
    /// Where the file is saved, or `None` if it never was.
    pub path: Option<PathBuf>,
    /// The tab index, which names files that were never saved.
    pub index: usize,
    /// The full text. Cloning a rope is cheap, so the editor hands it over without copying.
    pub text: Rope,
    /// The editor's version of the text, so an unchanged file is not sent again.
    pub version: u64,
    /// The char offset of the cursor.
    pub cursor: usize,
    /// How many columns a tab takes.
    pub tab_size: usize,
    /// Whether indenting inserts spaces instead of tabs.
    pub insert_spaces: bool,
}

/// Gets each piece of a chat reply as it streams in.
pub type OnText<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// A tool the model may call while chatting, like one a plugin adds.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The name the model calls it by, letters, digits, `_` and `-`.
    pub name: String,
    /// What it does and when to use it, for the model.
    pub description: String,
    /// A JSON schema of its input, an object.
    pub input_schema: Value,
}

/// Runs the tool called `name` with `input`, giving its output or why it failed.
pub type CallTool<'a> =
    &'a (dyn Fn(String, Value) -> BoxFuture<'static, Result<String, String>> + Send + Sync);

/// An AI backend that can chat, complete code, or both.
pub trait AiProvider: Send + Sync {
    /// Returns a short unique name like `claude`.
    fn id(&self) -> &str;

    /// Sends a conversation with `system` instructions, passing the reply to `on_text` as it
    /// arrives, and returns the whole reply text.
    fn chat<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [ChatMessage],
        on_text: OnText<'a>,
    ) -> BoxFuture<'a, Result<String, AiError>> {
        let _ = (system, messages, on_text);
        Box::pin(async move { Err(AiError::Unsupported(format!("{} chat", self.id()))) })
    }

    /// Chats like [`AiProvider::chat`], letting the model call `tools` through `call` as often
    /// as it needs before it answers. Providers without tool use just chat.
    fn chat_with_tools<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [ChatMessage],
        tools: &'a [Tool],
        call: CallTool<'a>,
        on_text: OnText<'a>,
    ) -> BoxFuture<'a, Result<String, AiError>> {
        let _ = (tools, call);
        self.chat(system, messages, on_text)
    }

    /// Suggests texts to insert at the cursor, best first, or none if nothing is worth suggesting.
    fn complete<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> BoxFuture<'a, Result<Vec<String>, AiError>> {
        let _ = request;
        Box::pin(async move { Err(AiError::Unsupported(format!("{} completion", self.id()))) })
    }
}
