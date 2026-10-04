//! AI providers for mog.
//!
//! Every backend implements [`AiProvider`] so the editor does not care which one is answering.

pub mod provider;

pub use provider::{AiError, AiProvider, BoxFuture, ChatMessage, CompletionRequest, Role};
