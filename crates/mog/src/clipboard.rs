//! The system clipboard.

use arboard::Clipboard as Arboard;
use mog_core::{Clipboard, MemoryClipboard};

/// The operating system clipboard.
struct SystemClipboard {
    /// The handle to the system clipboard.
    inner: Arboard,
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        self.inner.get_text().ok()
    }

    fn set(&mut self, text: String) {
        // a failed copy is not worth interrupting the user for
        let _ = self.inner.set_text(text);
    }
}

/// Returns the system clipboard, or an in memory one if it cannot be opened.
pub fn open() -> Box<dyn Clipboard> {
    match Arboard::new() {
        Ok(inner) => Box::new(SystemClipboard { inner }),
        Err(_) => Box::new(MemoryClipboard::default()),
    }
}
