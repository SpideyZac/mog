//! GitHub Copilot, not wired up yet.
//!
//! Copilot needs a GitHub device sign in and its own completion protocol. Until that exists this
//! provider is registered so config and commands can refer to it, and every call reports that it
//! is unsupported.

use crate::provider::{AiError, AiProvider, BoxFuture, ChatMessage, CompletionRequest};

/// The placeholder Copilot provider.
#[derive(Debug, Default)]
pub struct Copilot;

impl Copilot {
    /// Creates the provider.
    pub fn new() -> Self {
        Self
    }
}

impl AiProvider for Copilot {
    fn id(&self) -> &str {
        "copilot"
    }

    fn chat<'a>(&'a self, _messages: &'a [ChatMessage]) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async { Err(AiError::Unsupported("copilot".into())) })
    }

    fn complete<'a>(
        &'a self,
        _request: &'a CompletionRequest,
    ) -> BoxFuture<'a, Result<Option<String>, AiError>> {
        Box::pin(async { Err(AiError::Unsupported("copilot".into())) })
    }
}
