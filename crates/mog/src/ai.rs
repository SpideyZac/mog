//! Running AI requests in the background.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::future;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use mog_ai::{
    BoxFuture, ChatMessage, CompletionRequest, Copilot, CopilotEvent, CopilotStatus, DeviceCode,
    Role, Tool,
};
use mog_plugin::{Plugin, PluginTool};
use serde_json::{Value, json};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time,
};

use crate::settings::AiProviders;

/// The instructions sent with every chat.
const CHAT_SYSTEM: &str = "You are the assistant inside mog, a terminal code editor. Keep answers \
short and practical. Plain text renders best, avoid big markdown tables.";

/// The most chars of conversation sent with a chat, counted from the newest message back.
const MAX_CHAT_CHARS: usize = 400_000;

/// How long a plugin tool may run for the chat.
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

/// A plugin tool offered to the chat.
#[derive(Debug, Clone)]
pub struct ChatTool {
    /// The name the model calls it by.
    pub name: String,
    /// The tool as the plugin described it.
    pub tool: PluginTool,
    /// The plugin that runs it.
    pub plugin: Plugin,
}

/// Runs `tool` with `input` through `tool/call`, giving its text or why it failed.
pub async fn call_tool(tool: ChatTool, input: Value) -> Result<String, String> {
    let params = json!({ "name": tool.tool.name, "input": input });
    let answer = tool
        .plugin
        .request("tool/call", params, TOOL_TIMEOUT)
        .await?;
    let content = match &answer["content"] {
        Value::String(text) => text.clone(),
        Value::Null => answer.to_string(),
        other => other.to_string(),
    };
    if answer["is_error"].as_bool().unwrap_or(false) {
        Err(content)
    } else {
        Ok(content)
    }
}

/// How long typing has to pause before a ghost suggestion is requested.
const GHOST_DELAY: Duration = Duration::from_millis(650);

/// Turns the chat `history`, as `(from_user, text)` pairs, into messages that fit in `budget`
/// chars, dropping the oldest first.
///
/// The conversation always starts with something the user said, as the API wants.
fn chat_messages(history: &[(bool, String)], budget: usize) -> Vec<ChatMessage> {
    let mut used = 0;
    let mut start = history.len();
    for (index, (_, text)) in history.iter().enumerate().rev() {
        if used + text.len() > budget && start < history.len() {
            break;
        }
        used += text.len();
        start = index;
    }
    while history.get(start).is_some_and(|(from_user, _)| !from_user) {
        start += 1;
    }
    history[start..]
        .iter()
        .map(|(from_user, text)| ChatMessage {
            role: if *from_user {
                Role::User
            } else {
                Role::Assistant
            },
            text: text.clone(),
        })
        .collect()
}

/// Decides which files are never sent to an AI.
pub struct Exclusions {
    /// The patterns, rooted at the project.
    matcher: Gitignore,
}

impl Exclusions {
    /// Builds the exclusions from gitignore style `patterns` for the project at `root`.
    ///
    /// Patterns that do not parse are skipped.
    pub fn new(root: &Path, patterns: &[String]) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        for pattern in patterns {
            let _ = builder.add_line(None, pattern);
        }
        Self {
            matcher: builder.build().unwrap_or_else(|_| Gitignore::empty()),
        }
    }

    /// Returns whether `path` must not be sent to an AI.
    pub fn excludes(&self, path: &Path) -> bool {
        if path.starts_with(self.matcher.path()) {
            return self
                .matcher
                .matched_path_or_any_parents(path, false)
                .is_ignore();
        }
        // outside the project only the name can match
        path.file_name()
            .is_some_and(|name| self.matcher.matched(Path::new(name), false).is_ignore())
    }
}

/// What a finished request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiReply {
    /// More of the chat answer that is streaming in.
    ChatText(String),
    /// The whole chat answer, once it is done.
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
    /// Files never sent to an AI.
    exclusions: Exclusions,
    /// What the Copilot server reports.
    copilot_events: Option<UnboundedReceiver<CopilotEvent>>,
}

impl Assistant {
    /// Creates an assistant over `providers` that never sends files matching `exclusions`.
    pub fn new(mut providers: AiProviders, exclusions: Exclusions) -> Self {
        let (sender, replies) = mpsc::unbounded_channel();
        Self {
            copilot: providers.copilot.take(),
            copilot_events: providers.copilot_events.take(),
            providers,
            sender,
            replies,
            ghost_generation: Arc::new(AtomicU64::new(0)),
            exclusions,
        }
    }

    /// Returns whether `path` must not be sent to an AI.
    pub fn excludes(&self, path: &Path) -> bool {
        self.exclusions.excludes(path)
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

    /// Sends the conversation `history`, as `(from_user, text)` pairs, to the preferred provider,
    /// which may call plugin `tools` while it answers.
    ///
    /// Returns `false` if no provider is enabled for chat.
    pub fn chat(&self, history: &[(bool, String)], tools: Vec<ChatTool>) -> bool {
        let Some(provider) = self.providers.chat.first().cloned() else {
            return false;
        };
        let messages = chat_messages(history, MAX_CHAT_CHARS);
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let streamed = sender.clone();
            let on_text = move |text: &str| {
                let _ = streamed.send(AiReply::ChatText(text.to_owned()));
            };
            let described: Vec<Tool> = tools
                .iter()
                .map(|tool| Tool {
                    name: tool.name.clone(),
                    description: tool.tool.description.clone(),
                    input_schema: tool.tool.input_schema.clone(),
                })
                .collect();
            let call =
                move |name: String, input: Value| -> BoxFuture<'static, Result<String, String>> {
                    let tool = tools.iter().find(|tool| tool.name == name).cloned();
                    Box::pin(async move {
                        match tool {
                            Some(tool) => call_tool(tool, input).await,
                            None => Err(format!("there is no tool called {name}")),
                        }
                    })
                };
            let answer = provider
                .chat_with_tools(CHAT_SYSTEM, &messages, &described, &call, &on_text)
                .await;
            let reply = match answer {
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

#[cfg(test)]
/// Tests for the assistant.
mod tests {
    use std::path::Path;

    use mog_ai::Role;
    use mog_config::AiConfig;

    use super::{Exclusions, chat_messages};

    /// Secret files are kept from the ai, in and out of the project, and other files are not.
    #[test]
    fn excludes_secrets() {
        let root = Path::new("/code/app");
        let exclusions = Exclusions::new(root, &AiConfig::default().exclude);
        assert!(exclusions.excludes(&root.join(".env")));
        assert!(exclusions.excludes(&root.join("config/.env.local")));
        assert!(exclusions.excludes(&root.join("certs/server.pem")));
        assert!(exclusions.excludes(Path::new("/home/me/.ssh/id_rsa")));
        assert!(!exclusions.excludes(&root.join("src/main.rs")));
        let custom = Exclusions::new(root, &["private/".to_owned()]);
        assert!(custom.excludes(&root.join("private/notes.md")));
        assert!(!custom.excludes(&root.join(".env")));
    }

    /// Long chats drop their oldest messages and still start with the user.
    #[test]
    fn trims_chat_history() {
        let history = [
            (true, "a".repeat(10)),
            (false, "b".repeat(10)),
            (true, "c".repeat(10)),
            (false, "d".repeat(10)),
            (true, "e".repeat(10)),
        ];
        assert_eq!(chat_messages(&history, 100).len(), 5);
        let trimmed = chat_messages(&history, 35);
        assert_eq!(trimmed.len(), 3);
        assert_eq!(trimmed[0].role, Role::User);
        assert!(trimmed[0].text.starts_with('c'));
        let tiny = chat_messages(&history, 1);
        assert_eq!(tiny.len(), 1);
        assert!(tiny[0].text.starts_with('e'));
    }
}
