//! Running AI requests in the background.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::future;
use mog_ai::{
    ChatMessage, CompletionRequest, Copilot, CopilotEvent, CopilotStatus, DeviceCode, Role,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time,
};

use crate::settings::AiProviders;

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
    /// Texts to suggest at `pos` of document `document` at `version`.
    Ghost {
        /// The document index.
        document: usize,
        /// The document version the suggestions were made for.
        version: u64,
        /// Where the suggestions go.
        pos: usize,
        /// The suggested texts, best first.
        items: Vec<String>,
        /// Whether these were asked for to cycle through, so they add to the shown ones.
        more: bool,
    },
    /// Something the Copilot server reported.
    Copilot(CopilotEvent),
    /// A Copilot sign in that waits for the user to enter a code on GitHub.
    CopilotCode(DeviceCode),
    /// A Copilot account action finished, with a message and the new status if it changed.
    CopilotDone(String, Option<CopilotStatus>),
}

/// Sends requests to the configured providers and collects their replies.
pub struct Assistant {
    /// The enabled providers, split by feature.
    providers: AiProviders,
    /// Where finished requests send their reply.
    sender: UnboundedSender<AiReply>,
    /// Finished replies waiting to be shown.
    replies: UnboundedReceiver<AiReply>,
    /// Bumped on every ghost request so stale ones give up.
    ghost_generation: Arc<AtomicU64>,
    /// Copilot, if it is running.
    copilot: Option<Arc<Copilot>>,
    /// What the Copilot server reports.
    copilot_events: Option<UnboundedReceiver<CopilotEvent>>,
}

impl Assistant {
    /// Creates an assistant over `providers`.
    pub fn new(mut providers: AiProviders) -> Self {
        let (sender, replies) = mpsc::unbounded_channel();
        Self {
            copilot: providers.copilot.take(),
            copilot_events: providers.copilot_events.take(),
            providers,
            sender,
            replies,
            ghost_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Starts signing in to Copilot. The answer is a [`AiReply::CopilotCode`] or, if someone is
    /// already signed in, a [`AiReply::CopilotDone`].
    pub fn copilot_sign_in(&self) {
        self.copilot_task(|copilot| async move {
            match copilot.sign_in().await {
                Ok(Ok(code)) => AiReply::CopilotCode(code),
                Ok(Err(user)) => AiReply::CopilotDone(
                    format!("copilot is already signed in as {user}"),
                    Some(CopilotStatus::Ready),
                ),
                Err(err) => AiReply::CopilotDone(format!("copilot sign in failed: {err}"), None),
            }
        });
    }

    /// Finishes a Copilot sign in once the user is ready to enter `code` on GitHub.
    pub fn copilot_finish_sign_in(&self, code: DeviceCode) {
        self.copilot_task(|copilot| async move {
            match copilot.finish_sign_in(&code).await {
                Ok(user) => AiReply::CopilotDone(
                    format!("signed in to copilot as {user}"),
                    Some(CopilotStatus::Ready),
                ),
                Err(err) => AiReply::CopilotDone(format!("copilot sign in failed: {err}"), None),
            }
        });
    }

    /// Signs out of Copilot.
    pub fn copilot_sign_out(&self) {
        self.copilot_task(|copilot| async move {
            match copilot.sign_out().await {
                Ok(()) => AiReply::CopilotDone(
                    "signed out of copilot".into(),
                    Some(CopilotStatus::SignedOut),
                ),
                Err(err) => AiReply::CopilotDone(format!("copilot sign out failed: {err}"), None),
            }
        });
    }

    /// Asks Copilot who is signed in.
    pub fn copilot_check(&self) {
        self.copilot_task(|copilot| async move {
            match copilot.check().await {
                Ok((status, user)) => {
                    let message = match (&status, user) {
                        (CopilotStatus::Ready, Some(user)) => {
                            format!("copilot is ready, signed in as {user}")
                        }
                        (CopilotStatus::Problem(problem), _) => format!("copilot: {problem}"),
                        _ => "copilot is signed out, run Copilot: Sign in".into(),
                    };
                    AiReply::CopilotDone(message, Some(status))
                }
                Err(err) => AiReply::CopilotDone(format!("copilot check failed: {err}"), None),
            }
        });
    }

    /// Runs `task` with Copilot in the background and sends its reply.
    fn copilot_task<F, Fut>(&self, task: F)
    where
        F: FnOnce(Arc<Copilot>) -> Fut,
        Fut: Future<Output = AiReply> + Send + 'static,
    {
        let Some(copilot) = self.copilot.clone() else {
            let _ = self.sender.send(AiReply::CopilotDone(
                "copilot is off, set [ai.copilot] enabled = true in the config".into(),
                None,
            ));
            return;
        };
        let reply = task(copilot);
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let _ = sender.send(reply.await);
        });
    }

    /// Returns whether any provider is allowed to suggest ghost text.
    pub fn can_suggest(&self) -> bool {
        !self.providers.ghost.is_empty()
    }

    /// Sends the conversation `history`, as `(from_user, text)` pairs, to the preferred provider.
    ///
    /// Returns `false` if no provider is enabled for chat.
    pub fn chat(&self, history: &[(bool, String)]) -> bool {
        let Some(provider) = self.providers.chat.first().cloned() else {
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
    ///
    /// Requests the user `invoked` skip the pause.
    pub fn suggest(&self, request: CompletionRequest, document: usize, version: u64, pos: usize) {
        let Some(provider) = self.providers.ghost.first().cloned() else {
            return;
        };
        let generation = self.ghost_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = Arc::clone(&self.ghost_generation);
        let sender = self.sender.clone();
        tokio::spawn(async move {
            if !request.invoked {
                time::sleep(GHOST_DELAY).await;
            }
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }
            let Ok(mut items) = provider.complete(&request).await else {
                return;
            };
            items.retain(|text| !text.trim().is_empty());
            if !items.is_empty() && latest.load(Ordering::SeqCst) == generation {
                let _ = sender.send(AiReply::Ghost {
                    document,
                    version,
                    pos,
                    items,
                    more: request.invoked,
                });
            }
        });
    }

    /// Drops any pending ghost suggestion.
    pub fn cancel_suggestion(&self) {
        self.ghost_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Waits for the next finished reply or Copilot event.
    pub async fn reply(&mut self) -> Option<AiReply> {
        let copilot = async {
            match self.copilot_events.as_mut() {
                Some(events) => events.recv().await,
                None => future::pending().await,
            }
        };
        tokio::select! {
            reply = self.replies.recv() => reply,
            Some(event) = copilot => Some(AiReply::Copilot(event)),
        }
    }
}
