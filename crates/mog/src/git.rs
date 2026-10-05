//! Running git queries in the background and handing the answers to the ui.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use mog_git::{FileChange, FileStatus, Repo, repo};
use mog_tui::Ui;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task, time,
};

/// How long the cursor has to rest on a line before it is blamed.
const BLAME_DELAY: Duration = Duration::from_millis(300);

/// An answer from a background git query.
pub enum GitUpdate {
    /// The current branch and how far it is from its upstream.
    Branch(Option<String>, Option<(usize, usize)>),
    /// The status of every changed file.
    Status(HashMap<PathBuf, FileStatus>),
    /// The committed text of a file, or `None` if it is not committed.
    Base(PathBuf, Option<String>),
    /// Who last changed a line, as `(file, line, description)`.
    Blame(PathBuf, usize, Option<String>),
    /// Every changed file with its staged and unstaged parts.
    Changes(Vec<FileChange>),
    /// The diff of a file, of what is staged if the flag is set.
    Diff(PathBuf, bool, String),
    /// A git command finished, with a message for the status line or why it failed.
    Done(Result<String, String>),
}

/// The git state of the project, kept fresh by background tasks.
pub struct Git {
    /// The repository, or `None` when the project is not in one.
    repo: Option<Repo>,
    /// Where background tasks send their answers.
    sender: UnboundedSender<GitUpdate>,
    /// Answers waiting to be applied.
    updates: UnboundedReceiver<GitUpdate>,
    /// Files whose committed text was already asked for.
    requested: HashSet<PathBuf>,
    /// The last line blame was asked for, as `(file, line, version)`.
    blamed: Option<(PathBuf, usize, u64)>,
    /// Bumped on every blame request so stale ones can give up.
    blame_generation: Arc<AtomicU64>,
}

impl Git {
    /// Looks for a repository at `root`.
    pub fn new(root: &Path) -> Self {
        let (sender, updates) = mpsc::unbounded_channel();
        Self {
            repo: Repo::discover(root),
            sender,
            updates,
            requested: HashSet::new(),
            blamed: None,
            blame_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Refreshes the branch and file statuses, and the committed text of open files.
    pub fn refresh(&mut self) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.requested.clear();
        let sender = self.sender.clone();
        task::spawn_blocking(move || {
            let _ = sender.send(GitUpdate::Branch(repo.branch(), repo.ahead_behind()));
            let _ = sender.send(GitUpdate::Status(repo.status()));
        });
    }

    /// Asks for the committed text of `path` unless it was already asked for.
    pub fn ensure_base(&mut self, path: &Path) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if !self.requested.insert(path.to_owned()) {
            return;
        }
        let sender = self.sender.clone();
        let path = path.to_owned();
        task::spawn_blocking(move || {
            let text = repo.head_text(&path);
            let _ = sender.send(GitUpdate::Base(path, text));
        });
    }

    /// Asks who last changed `line` of `path` once the cursor rests there.
    ///
    /// `contents` is only called when the line or text changed since the last request.
    pub fn request_blame(
        &mut self,
        path: &Path,
        line: usize,
        version: u64,
        contents: impl FnOnce() -> String,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let key = (path.to_owned(), line, version);
        if self.blamed.as_ref() == Some(&key) {
            return;
        }
        self.blamed = Some(key);
        let contents = contents();
        let generation = self.blame_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = Arc::clone(&self.blame_generation);
        let sender = self.sender.clone();
        let path = path.to_owned();
        tokio::spawn(async move {
            time::sleep(BLAME_DELAY).await;
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }
            let blame = task::spawn_blocking(move || {
                let text = repo
                    .blame(&path, line, &contents)
                    .map(|blame| blame.describe(repo::now()));
                (path, text)
            })
            .await;
            if let Ok((path, text)) = blame {
                let _ = sender.send(GitUpdate::Blame(path, line, text));
            }
        });
    }

    /// Returns whether the project is in a repository.
    pub fn is_repo(&self) -> bool {
        self.repo.is_some()
    }

    /// Asks for every changed file, for the source control panel.
    pub fn request_changes(&self) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let sender = self.sender.clone();
        task::spawn_blocking(move || {
            let _ = sender.send(GitUpdate::Changes(repo.changes()));
        });
    }

    /// Asks for the diff of `path`, of what is staged if `staged` is set.
    pub fn request_diff(&self, path: PathBuf, staged: bool) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let sender = self.sender.clone();
        task::spawn_blocking(move || {
            let diff = repo.diff(&path, staged);
            let _ = sender.send(GitUpdate::Diff(path, staged, diff));
        });
    }

    /// Runs `op` on the repository in the background and reports how it went.
    pub fn run<F>(&self, op: F)
    where
        F: FnOnce(&Repo) -> Result<String, String> + Send + 'static,
    {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let sender = self.sender.clone();
        task::spawn_blocking(move || {
            let _ = sender.send(GitUpdate::Done(op(&repo)));
        });
    }

    /// Waits for the next background answer.
    pub async fn update(&mut self) -> Option<GitUpdate> {
        self.updates.recv().await
    }
}

/// Stores a background answer in the ui.
pub fn apply(ui: &mut Ui, update: GitUpdate) {
    match update {
        GitUpdate::Branch(branch, distance) => {
            ui.branch = branch.map(|name| match distance {
                Some((ahead, behind)) if ahead + behind > 0 => {
                    format!("{name} \u{2191}{ahead}\u{2193}{behind}")
                }
                _ => name,
            });
        }
        GitUpdate::Status(status) => ui.git_status = status,
        GitUpdate::Base(path, Some(text)) => {
            ui.git_base.insert(path, text);
        }
        GitUpdate::Base(path, None) => {
            ui.git_base.remove(&path);
        }
        GitUpdate::Blame(path, line, Some(text)) => ui.blame = Some((path, line, text)),
        GitUpdate::Blame(..) => ui.blame = None,
        GitUpdate::Changes(changes) => ui.git_panel.set_changes(changes),
        GitUpdate::Diff(path, staged, text) => ui.git_panel.set_diff(path, staged, &text),
        // the app reports these itself since they need the status line
        GitUpdate::Done(_) => {}
    }
}
