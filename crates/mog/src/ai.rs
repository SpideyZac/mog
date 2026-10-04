//! Running AI requests in the background.

use std::sync::Arc;

use mog_ai::{AiProvider, ChatMessage, Role};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The prompt sent before the selection for `ai.explain`.
const EXPLAIN_PROMPT: &str = "Explain what this code does in one short sentence:\n\n";

/// Sends requests to the configured providers and collects their replies.
pub struct Assistant {
    /// The enabled providers, preferred first.
    providers: Vec<Arc<dyn AiProvider>>,
    /// Where finished requests send their reply.
    sender: UnboundedSender<String>,
    /// Finished replies waiting to be shown.
    replies: UnboundedReceiver<String>,
}

impl Assistant {
    /// Creates an assistant over `providers`.
    pub fn new(providers: Vec<Arc<dyn AiProvider>>) -> Self {
        let (sender, replies) = mpsc::unbounded_channel();
        Self {
            providers,
            sender,
            replies,
        }
    }

    /// Asks the preferred provider to explain `code`. The answer arrives through [`Self::reply`].
    ///
    /// Returns a message to show right away.
    pub fn explain(&self, code: String) -> String {
        let Some(provider) = self.providers.first().cloned() else {
            return "no ai provider is enabled, see the [ai] config section".into();
        };
        let sender = self.sender.clone();
        let id = provider.id().to_owned();
        let status = format!("asking {id}...");
        tokio::spawn(async move {
            let messages = [ChatMessage {
                role: Role::User,
                text: format!("{EXPLAIN_PROMPT}{code}"),
            }];
            let reply = match provider.chat(&messages).await {
                Ok(text) => format!("{id}: {}", text.trim().replace('\n', " ")),
                Err(err) => format!("{id}: {err}"),
            };
            let _ = sender.send(reply);
        });
        status
    }

    /// Waits for the next finished reply.
    pub async fn reply(&mut self) -> Option<String> {
        self.replies.recv().await
    }
}
