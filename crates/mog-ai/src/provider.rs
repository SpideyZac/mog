//! The [`AiProvider`] trait every AI backend implements.

use std::{future::Future, path::PathBuf, pin::Pin};

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
}

/// The whole file a [`CompletionRequest`] is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionFile {
    /// Where the file is saved.
    pub path: PathBuf,
    /// The full text.
    pub text: String,
    /// The char offset of the cursor.
    pub cursor: usize,
    /// How many columns a tab takes.
    pub tab_size: usize,
    /// Whether indenting inserts spaces instead of tabs.
    pub insert_spaces: bool,
}

/// An AI backend that can chat and complete code.
pub trait AiProvider: Send + Sync {
    /// Returns a short unique name like `claude`.
    fn id(&self) -> &str;

    /// Sends a conversation and returns the reply text.
    fn chat<'a>(&'a self, messages: &'a [ChatMessage]) -> BoxFuture<'a, Result<String, AiError>>;

    /// Suggests texts to insert at the cursor, best first, or none if nothing is worth suggesting.
    fn complete<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> BoxFuture<'a, Result<Vec<String>, AiError>>;
}
