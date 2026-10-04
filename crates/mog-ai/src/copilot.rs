//! GitHub Copilot through its official language server.
//!
//! The server is `@github/copilot-language-server` from npm. It handles the GitHub sign in and
//! answers `textDocument/inlineCompletion` for files it is kept in sync with.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use lsp_types::Range;
use mog_lsp::{Client, LspError, LspEvent, convert};
use ropey::Rope;
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::provider::{
    AiError, AiProvider, BoxFuture, ChatMessage, CompletionFile, CompletionRequest,
};

/// The server name used in events and errors.
const NAME: &str = "copilot";

/// The trigger kind for completions the user asked for, which gives several.
const TRIGGER_INVOKED: u8 = 1;

/// The trigger kind for completions asked for while typing.
const TRIGGER_AUTOMATIC: u8 = 2;

/// Where the device flow sends people when the server does not say.
const DEVICE_URL: &str = "https://github.com/login/device";

/// Whether Copilot can be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopilotStatus {
    /// The server is starting and has not said yet.
    Starting,
    /// Signed in and working.
    Ready,
    /// Nobody is signed in.
    SignedOut,
    /// Something is wrong, like a missing subscription or a network error.
    Problem(String),
}

/// Something the Copilot server did that the editor may care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopilotEvent {
    /// The status changed.
    Status(CopilotStatus),
    /// The server wants a page shown, like the GitHub sign in page.
    ShowDocument {
        /// The address to show.
        uri: String,
        /// Whether it belongs outside the editor.
        external: bool,
    },
    /// The server wants the user to read a message.
    Message(String),
    /// The server stopped, with its last words if it left any.
    Exited(Option<String>),
}

/// A sign in that waits for the user to enter a code on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The code to enter.
    pub user_code: String,
    /// The page to enter it on.
    pub uri: String,
    /// The command that finishes the sign in once the user is ready.
    command: Value,
}

/// GitHub Copilot.
pub struct Copilot {
    /// The connection to the language server.
    client: Client,
    /// The last version sent to the server for each file.
    files: Mutex<HashMap<PathBuf, i32>>,
}

impl Copilot {
    /// Starts the Copilot language server `command` for the project at `root`.
    ///
    /// Returns the provider and its events. The first status arrives once the server is up.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot be started, usually because it is not installed.
    pub fn start(
        command: &str,
        args: &[String],
        root: &Path,
    ) -> io::Result<(Self, UnboundedReceiver<CopilotEvent>)> {
        let options = json!({
            "editorInfo": { "name": "mog", "version": env!("CARGO_PKG_VERSION") },
            "editorPluginInfo": { "name": "mog-copilot", "version": env!("CARGO_PKG_VERSION") },
        });
        let (lsp_events, lsp_rx) = mpsc::unbounded_channel();
        let client = Client::start(NAME, command, args, root, &options, lsp_events)?;
        let (events, events_rx) = mpsc::unbounded_channel();
        let _ = events.send(CopilotEvent::Status(CopilotStatus::Starting));
        tokio::spawn(forward(lsp_rx, events.clone()));
        let checker = client.clone();
        tokio::spawn(async move {
            // requests wait for the handshake so this lands once the server is up
            if let Ok(result) = checker.request("checkStatus", json!({})).await {
                let _ = events.send(CopilotEvent::Status(status_of(&result)));
            }
        });
        let copilot = Self {
            client,
            files: Mutex::new(HashMap::new()),
        };
        Ok((copilot, events_rx))
    }

    /// Starts signing in. Returns the signed in user if someone already is.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails or is gone.
    pub async fn sign_in(&self) -> Result<Result<DeviceCode, String>, AiError> {
        let result = self.request("signIn", json!({})).await?;
        if let Some(user_code) = result["userCode"].as_str() {
            let command = if result["command"].is_object() {
                result["command"].clone()
            } else {
                json!({ "command": "github.copilot.finishDeviceFlow", "arguments": [] })
            };
            return Ok(Ok(DeviceCode {
                user_code: user_code.to_owned(),
                uri: result["verificationUri"]
                    .as_str()
                    .unwrap_or(DEVICE_URL)
                    .to_owned(),
                command,
            }));
        }
        Ok(Err(result["user"].as_str().unwrap_or("someone").to_owned()))
    }

    /// Finishes a sign in once the user is ready, which opens the GitHub page through a
    /// [`CopilotEvent::ShowDocument`]. Returns the signed in user.
    ///
    /// # Errors
    ///
    /// Returns an error if the sign in fails, is not finished in time or the server is gone.
    pub async fn finish_sign_in(&self, code: &DeviceCode) -> Result<String, AiError> {
        let params = json!({
            "command": code.command["command"],
            "arguments": code.command["arguments"].as_array().cloned().unwrap_or_default(),
        });
        let result = self.request("workspace/executeCommand", params).await?;
        match result["status"].as_str() {
            Some("OK" | "AlreadySignedIn") => {
                Ok(result["user"].as_str().unwrap_or("you").to_owned())
            }
            _ => Err(AiError::Api(format!("sign in did not finish: {result}"))),
        }
    }

    /// Signs out of GitHub.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails or is gone.
    pub async fn sign_out(&self) -> Result<(), AiError> {
        self.request("signOut", json!({})).await.map(|_| ())
    }

    /// Asks the server who is signed in. Returns the status and the user name, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails or is gone.
    pub async fn check(&self) -> Result<(CopilotStatus, Option<String>), AiError> {
        let result = self.request("checkStatus", json!({})).await?;
        let user = result["user"].as_str().map(str::to_owned);
        Ok((status_of(&result), user))
    }

    /// Sends a request and turns server errors into [`AiError`]s.
    async fn request(&self, method: &str, params: Value) -> Result<Value, AiError> {
        self.client
            .request(method, params)
            .await
            .map_err(|err| match err {
                LspError::Closed => AiError::Api("the copilot server stopped".into()),
                LspError::Server(value) => AiError::Api(
                    value["message"]
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned),
                ),
            })
    }

    /// Sends the text of `file` to the server and returns the version it now has.
    fn sync(&self, file: &CompletionFile) -> i32 {
        let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(version) = files.get_mut(&file.path) {
            *version += 1;
            self.client.did_change(&file.path, *version, &file.text);
            return *version;
        }
        self.client
            .did_open(&file.path, language_id(&file.path), 1, &file.text);
        files.insert(file.path.clone(), 1);
        1
    }
}

impl AiProvider for Copilot {
    fn id(&self) -> &str {
        NAME
    }

    fn chat<'a>(&'a self, _messages: &'a [ChatMessage]) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async { Err(AiError::Unsupported("copilot chat".into())) })
    }

    fn complete<'a>(
        &'a self,
        request: &'a CompletionRequest,
    ) -> BoxFuture<'a, Result<Vec<String>, AiError>> {
        Box::pin(async move {
            let Some(file) = &request.file else {
                return Ok(Vec::new());
            };
            let Some(uri) = convert::path_to_uri(&file.path) else {
                return Ok(Vec::new());
            };
            let version = self.sync(file);
            let text = Rope::from_str(&file.text);
            let params = json!({
                "textDocument": { "uri": uri, "version": version },
                "position": convert::char_to_position(&text, file.cursor),
                "context": {
                    "triggerKind": if request.invoked { TRIGGER_INVOKED } else { TRIGGER_AUTOMATIC },
                },
                "formattingOptions": {
                    "tabSize": file.tab_size,
                    "insertSpaces": file.insert_spaces,
                },
            });
            let result = self
                .request("textDocument/inlineCompletion", params)
                .await?;
            let items = result["items"]
                .as_array()
                .or_else(|| result.as_array())
                .cloned()
                .unwrap_or_default();
            let mut suggestions: Vec<String> = Vec::new();
            for item in &items {
                if let Some(suggestion) = suggestion_at(&text, file.cursor, item)
                    && !suggestions.contains(&suggestion)
                {
                    suggestions.push(suggestion);
                }
            }
            Ok(suggestions)
        })
    }
}

/// Turns server events into [`CopilotEvent`]s until the server goes away.
async fn forward(mut lsp: UnboundedReceiver<LspEvent>, events: UnboundedSender<CopilotEvent>) {
    while let Some(event) = lsp.recv().await {
        let event = match event {
            LspEvent::Notification { method, params, .. } if method == "didChangeStatus" => {
                CopilotEvent::Status(status_from_notification(&params))
            }
            LspEvent::ShowDocument { uri, external, .. } => {
                CopilotEvent::ShowDocument { uri, external }
            }
            LspEvent::Message { text, .. } => CopilotEvent::Message(text),
            LspEvent::Exited { reason, .. } => CopilotEvent::Exited(reason),
            _ => continue,
        };
        if events.send(event).is_err() {
            break;
        }
    }
}

/// Reads the status out of a `checkStatus` or `signIn` result.
fn status_of(result: &Value) -> CopilotStatus {
    match result["status"].as_str() {
        Some("OK" | "AlreadySignedIn" | "MaybeOk") => CopilotStatus::Ready,
        Some("NotAuthorized") => CopilotStatus::Problem(format!(
            "{} has no copilot subscription",
            result["user"].as_str().unwrap_or("this account")
        )),
        _ => CopilotStatus::SignedOut,
    }
}

/// Reads the status out of a `didChangeStatus` notification.
fn status_from_notification(params: &Value) -> CopilotStatus {
    let message = params["message"].as_str().unwrap_or_default();
    let wants_sign_in = params["command"]["command"] == "github.copilot.signIn";
    match params["kind"].as_str() {
        Some("Normal") => CopilotStatus::Ready,
        Some("Error") if wants_sign_in => CopilotStatus::SignedOut,
        _ if message.is_empty() => CopilotStatus::Problem("copilot is not working".into()),
        _ => CopilotStatus::Problem(message.to_owned()),
    }
}

/// Turns a completion item into the text to insert at `cursor`.
///
/// Items replace a range around the cursor, usually the whole line, so the text already before
/// and after the cursor is cut off. Items that do not fit that shape are dropped.
fn suggestion_at(text: &Rope, cursor: usize, item: &Value) -> Option<String> {
    let insert = item["insertText"]
        .as_str()
        .or_else(|| item["insertText"]["value"].as_str())?;
    let (start, end) = match serde_json::from_value::<Range>(item["range"].clone()) {
        Ok(range) => (
            convert::position_to_char(text, range.start),
            convert::position_to_char(text, range.end),
        ),
        Err(_) => (cursor, cursor),
    };
    if start > cursor || end < cursor {
        return None;
    }
    let before = text.slice(start..cursor).to_string();
    let after = text.slice(cursor..end).to_string();
    let middle = insert.strip_prefix(&before)?.strip_suffix(&after)?;
    (!middle.trim().is_empty()).then(|| middle.to_owned())
}

/// Returns the language id Copilot expects for the file at `path`.
fn language_id(path: &Path) -> &str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    match ext {
        "" => "plaintext",
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "md" => "markdown",
        "sh" | "bash" => "shellscript",
        "yml" => "yaml",
        "kt" => "kotlin",
        ext => ext,
    }
}

#[cfg(test)]
/// Tests for reading Copilot answers.
mod tests {
    use std::path::Path;

    use ropey::Rope;
    use serde_json::json;

    use super::{CopilotStatus, language_id, status_from_notification, status_of, suggestion_at};

    /// The typed start of the line and the text after the cursor are cut off.
    #[test]
    fn trims_replaced_range() {
        let text = Rope::from_str("fn main() {\n    print()\n}\n");
        // the cursor sits between the parens of print()
        let cursor = text.line_to_char(1) + 10;
        let item = json!({
            "insertText": "    print(\"hi\")",
            "range": {
                "start": { "line": 1, "character": 0 },
                "end": { "line": 1, "character": 11 },
            },
        });
        assert_eq!(
            suggestion_at(&text, cursor, &item).as_deref(),
            Some("\"hi\"")
        );
    }

    /// Items that do not match the typed text are dropped.
    #[test]
    fn drops_mismatched_items() {
        let text = Rope::from_str("let x = 1;\n");
        let item = json!({
            "insertText": "const y = 2;",
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 4 },
            },
        });
        assert_eq!(suggestion_at(&text, 4, &item), None);
    }

    /// Items without a range insert at the cursor.
    #[test]
    fn inserts_without_range() {
        let text = Rope::from_str("let x");
        let item = json!({ "insertText": " = 1;" });
        assert_eq!(suggestion_at(&text, 5, &item).as_deref(), Some(" = 1;"));
    }

    /// Statuses are read from check results and notifications.
    #[test]
    fn reads_statuses() {
        assert_eq!(
            status_of(&json!({ "status": "OK", "user": "a" })),
            CopilotStatus::Ready
        );
        assert_eq!(
            status_of(&json!({ "status": "NotSignedIn" })),
            CopilotStatus::SignedOut
        );
        let expired = json!({
            "kind": "Error",
            "message": "Your GitHub Copilot session has expired.",
            "command": { "command": "github.copilot.signIn", "title": "Sign In" },
        });
        assert_eq!(status_from_notification(&expired), CopilotStatus::SignedOut);
        let offline = json!({ "kind": "Warning", "message": "offline" });
        assert_eq!(
            status_from_notification(&offline),
            CopilotStatus::Problem("offline".into())
        );
    }

    /// Extensions map to the language ids VS Code uses.
    #[test]
    fn maps_language_ids() {
        assert_eq!(language_id(Path::new("a.rs")), "rust");
        assert_eq!(language_id(Path::new("a.tsx")), "typescriptreact");
        assert_eq!(language_id(Path::new("Makefile")), "plaintext");
    }
}
