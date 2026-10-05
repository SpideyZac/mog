//! Builders for the actions a plugin can ask mog for.

use serde_json::{Value, json};

/// Shows `text` in the status line.
pub fn status(text: impl Into<String>) -> Value {
    json!({ "type": "status", "text": text.into() })
}

/// Shows a message with a `level` of `info`, `warning` or `error`.
pub fn notify(text: impl Into<String>, level: &str) -> Value {
    json!({ "type": "notify", "text": text.into(), "level": level })
}

/// Replaces the selection, or types at the cursor.
pub fn insert(text: impl Into<String>) -> Value {
    json!({ "type": "insert", "text": text.into() })
}

/// One change for [`edit`]: replaces the chars `start..end` with `text`.
pub fn change(start: usize, end: usize, text: impl Into<String>) -> Value {
    json!({ "start": start, "end": end, "text": text.into() })
}

/// Changes the file at `path`, the focused one when `None`, in one undo step.
///
/// With a `version`, mog refuses the edit if the file changed since that version.
pub fn edit(changes: Vec<Value>, path: Option<&str>, version: Option<u64>) -> Value {
    json!({ "type": "edit", "changes": changes, "path": path, "version": version })
}

/// Opens a file, at a line and column from 0 if given.
pub fn open(path: &str, line: Option<usize>, column: Option<usize>) -> Value {
    json!({ "type": "open", "path": path, "line": line, "column": column })
}

/// Runs a mog command by name, with arguments for plugin commands.
pub fn command(name: &str, args: Value) -> Value {
    json!({ "type": "command", "name": name, "args": args })
}

/// Shows `text` in the output panel under `title`.
pub fn output(title: &str, text: impl Into<String>) -> Value {
    json!({ "type": "output", "title": title, "text": text.into() })
}
