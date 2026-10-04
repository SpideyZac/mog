//! AI providers for mog.
//!
//! Every backend implements [`AiProvider`] so the editor does not care which one is answering.

pub mod claude;
pub mod provider;

pub use claude::Claude;
pub use provider::{AiError, AiProvider, BoxFuture, ChatMessage, CompletionRequest, Role};
