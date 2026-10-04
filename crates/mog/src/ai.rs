//! Running AI requests in the background.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use mog_ai::{AiProvider, ChatMessage, CompletionRequest, Role};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time,
};

/// The instructions sent with every chat.
const CHAT_SYSTEM: &str = "You are the assistant inside mog, a terminal code editor. Keep answers \
short and practical. Plain text renders best, avoid big markdown tables.";

/// How long typing has to pause before a ghost suggestion is requested.
const GHOST_DELAY: Duration = Duration::from_millis(650);

/// What a finished request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiReply {
    /// A chat answer.
    Chat(String),
    /// A chat that failed, with the reason.
    ChatFailed(String),
    /// Text to suggest at `pos` of document `document` at `version`.
    Ghost {
        /// The document index.
        document: usize,
        /// The document version the suggestion was made for.
        version: u64,
        /// Where the suggestion goes.
        pos: usize,
        /// The suggested text.
        text: String,
    },
}

/// Sends requests to the configured providers and collects their replies.
pub struct Assistant {
    /// The enabled providers, preferred first.
    providers: Vec<Arc<dyn AiProvider>>,
    /// Where finished requests send their reply.
    sender: UnboundedSender<AiReply>,
    /// Finished replies waiting to be shown.
    replies: UnboundedReceiver<AiReply>,
    /// Bumped on every ghost request so stale ones give up.
    ghost_generation: Arc<AtomicU64>,
}

impl Assistant {
    /// Creates an assistant over `providers`.
    pub fn new(providers: Vec<Arc<dyn AiProvider>>) -> Self {
        let (sender, replies) = mpsc::unbounded_channel();
        Self {
            providers,
            sender,
            replies,
            ghost_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns the name of the preferred provider, if any is enabled.
    pub fn provider_name(&self) -> Option<&str> {
        self.providers.first().map(|provider| provider.id())
    }

    /// Sends the conversation `history`, as `(from_user, text)` pairs, to the preferred provider.
    ///
    /// Returns `false` if no provider is enabled.
    pub fn chat(&self, history: &[(bool, String)]) -> bool {
        let Some(provider) = self.providers.first().cloned() else {
            return false;
        };
        let messages: Vec<ChatMessage> = history
            .iter()
            .enumerate()
            .map(|(i, (from_user, text))| ChatMessage {
                role: if *from_user {
                    Role::User
                } else {
                    Role::Assistant
                },
                // the provider takes no system prompt so the first message carries it
                text: if i == 0 {
                    format!("{CHAT_SYSTEM}\n\n{text}")
                } else {
                    text.clone()
                },
            })
            .collect();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let reply = match provider.chat(&messages).await {
                Ok(text) => AiReply::Chat(text.trim().to_owned()),
                Err(err) => AiReply::ChatFailed(err.to_string()),
            };
            let _ = sender.send(reply);
        });
        true
    }

    /// Asks for a ghost suggestion once typing pauses, replacing any pending request.
    pub fn suggest(&self, request: CompletionRequest, document: usize, version: u64, pos: usize) {
        let Some(provider) = self.providers.first().cloned() else {
            return;
        };
        let generation = self.ghost_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = Arc::clone(&self.ghost_generation);
        let sender = self.sender.clone();
        tokio::spawn(async move {
            time::sleep(GHOST_DELAY).await;
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }
            if let Ok(Some(text)) = provider.complete(&request).await
                && !text.trim().is_empty()
                && latest.load(Ordering::SeqCst) == generation
            {
                let _ = sender.send(AiReply::Ghost {
                    document,
                    version,
                    pos,
                    text,
                });
            }
        });
    }

    /// Drops any pending ghost suggestion.
    pub fn cancel_suggestion(&self) {
        self.ghost_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Waits for the next finished reply.
    pub async fn reply(&mut self) -> Option<AiReply> {
        self.replies.recv().await
    }
}
