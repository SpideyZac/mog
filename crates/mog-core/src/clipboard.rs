//! Clipboard access.

/// Somewhere copied text goes.
///
/// The core uses this trait so it does not depend on the system clipboard. The binary plugs in
/// the real one.
pub trait Clipboard: Send {
    /// Returns the clipboard text, if there is any.
    fn get(&mut self) -> Option<String>;

    /// Replaces the clipboard text.
    fn set(&mut self, text: String);
}

/// A clipboard that only lives in memory, used when the system clipboard is unavailable.
#[derive(Debug, Clone, Default)]
pub struct MemoryClipboard {
    /// The stored text.
    text: Option<String>,
}

impl Clipboard for MemoryClipboard {
    fn get(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set(&mut self, text: String) {
        self.text = Some(text);
    }
}
