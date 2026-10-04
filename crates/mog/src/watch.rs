//! Noticing files that change on disk.

use std::{path::Path, sync::Arc};

use notify::{
    Event as FsEvent, RecommendedWatcher, RecursiveMode, Result as WatchResult, Watcher,
    recommended_watcher,
};
use tokio::sync::Notify;

/// Folder names whose changes are too noisy to care about.
const IGNORED: [&str; 3] = ["target", "node_modules", ".git"];

/// Watches a folder and signals when anything in it changes.
pub struct FolderWatcher {
    /// The watcher, kept alive for as long as changes are wanted.
    _watcher: RecommendedWatcher,
    /// Woken once for any burst of changes.
    changed: Arc<Notify>,
}

impl FolderWatcher {
    /// Starts watching `root`, or returns `None` if the platform watcher fails.
    pub fn new(root: &Path) -> Option<Self> {
        let changed = Arc::new(Notify::new());
        let signal = Arc::clone(&changed);
        let mut watcher = recommended_watcher(move |event: WatchResult<FsEvent>| {
            let Ok(event) = event else {
                return;
            };
            let interesting = event.paths.iter().any(|path| {
                // the git index changing means a commit or stage happened, which matters
                path.ends_with(".git/index")
                    || !path
                        .components()
                        .any(|part| IGNORED.contains(&part.as_os_str().to_string_lossy().as_ref()))
            });
            if interesting {
                signal.notify_one();
            }
        })
        .ok()?;
        watcher.watch(root, RecursiveMode::Recursive).ok()?;
        Some(Self {
            _watcher: watcher,
            changed,
        })
    }

    /// Waits until something changed since the last call.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }
}
