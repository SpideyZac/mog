//! Remembering open files between runs, unsaved work after a crash and undo history.
//!
//! Everything lives as JSON under [`state_dir`], in `sessions`, `swap` and `undo` folders, with
//! file names hashed from the project or file they are for.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use mog_config::state_dir;
use mog_core::History;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// The most undo steps kept for one file.
const MAX_UNDO: usize = 500;

/// Files bigger than this many bytes do not get their undo history saved.
const MAX_UNDO_FILE: usize = 4 << 20;

/// One open file in a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFile {
    /// The file.
    pub path: PathBuf,
    /// Where the selection starts.
    pub anchor: usize,
    /// Where the cursor is.
    pub head: usize,
    /// The first visible line.
    pub scroll_line: usize,
}

/// What was open in a project, to bring back next time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The open files in tab order.
    pub files: Vec<SessionFile>,
    /// The index of the focused file.
    pub active: usize,
    /// The index of the file in the other pane, if the editor was split.
    pub split: Option<usize>,
    /// Whether the file explorer was open.
    pub explorer_open: bool,
}

/// Unsaved text kept on disk so a crash does not lose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    /// The file the text belongs to, `None` for an untitled document.
    pub path: Option<PathBuf>,
    /// The project that was open.
    pub root: PathBuf,
    /// The process that wrote it, so a running mog's swaps are left alone.
    pub pid: u32,
    /// The unsaved text.
    pub text: String,
}

/// Undo history saved for one version of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedUndo {
    /// The hash of the text the history belongs to.
    hash: String,
    /// The undo and redo stacks.
    history: History,
}

/// Returns a short hex hash of `text`, used for file names and to compare contents.
fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Returns whether a process with `pid` is running.
fn is_running(pid: u32) -> bool {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(pid).is_some()
}

/// Reads JSON from `path`, or `None` if it is missing or broken.
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Writes `value` as JSON to `path`, creating folders as needed. Failures are ignored since all
/// of this is a convenience.
fn write_json<T: Serialize>(path: &Path, value: &T) {
    let Ok(text) = serde_json::to_string(value) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // write next to it and rename so a crash mid write never leaves half a file
    let temp = path.with_extension("tmp");
    if fs::write(&temp, text).is_ok() {
        let _ = fs::rename(&temp, path);
    }
}

/// Where sessions, swap files and undo history are kept.
#[derive(Debug, Clone)]
pub struct State {
    /// The state folder.
    dir: PathBuf,
}

impl State {
    /// Returns the state in the usual folder, if the system has one.
    pub fn open() -> Option<Self> {
        state_dir().map(|dir| Self { dir })
    }

    /// Returns the state kept in `dir`.
    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Returns the session file of the project at `root`.
    fn session_path(&self, root: &Path) -> PathBuf {
        let key = hash(&root.to_string_lossy());
        self.dir.join("sessions").join(format!("{key}.json"))
    }

    /// Returns the session saved for the project at `root`.
    pub fn load_session(&self, root: &Path) -> Option<Session> {
        read_json(&self.session_path(root))
    }

    /// Saves `session` for the project at `root`.
    pub fn save_session(&self, root: &Path, session: &Session) {
        write_json(&self.session_path(root), session);
    }

    /// Returns the swap file for the document called `key`, a path or an untitled name.
    pub fn swap_path(&self, key: &str) -> PathBuf {
        let pid = process::id();
        self.dir
            .join("swap")
            .join(format!("{}-{pid}.json", hash(key)))
    }

    /// Writes unsaved `text` for the document called `key` and returns where it went.
    pub fn write_swap(&self, key: &str, path: Option<&Path>, root: &Path, text: String) -> PathBuf {
        let file = self.swap_path(key);
        let swap = Swap {
            path: path.map(ToOwned::to_owned),
            root: root.to_owned(),
            pid: process::id(),
            text,
        };
        write_json(&file, &swap);
        file
    }

    /// Returns the swap files left behind for the project at `root` by a mog that is gone.
    pub fn orphaned_swaps(&self, root: &Path) -> Vec<(PathBuf, Swap)> {
        let Ok(entries) = fs::read_dir(self.dir.join("swap")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| file.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|file| Some((file.clone(), read_json::<Swap>(&file)?)))
            .filter(|(_, swap)| swap.root == root && !is_running(swap.pid))
            .collect()
    }

    /// Returns the undo history file for the file at `path`.
    fn undo_path(&self, path: &Path) -> PathBuf {
        let key = hash(&path.to_string_lossy());
        self.dir.join("undo").join(format!("{key}.json"))
    }

    /// Saves `history` for the file at `path` whose text is now `text`.
    pub fn save_undo(&self, path: &Path, text: &str, history: &History) {
        if history.is_empty() || text.len() > MAX_UNDO_FILE {
            return;
        }
        let mut history = history.clone();
        history.truncate(MAX_UNDO);
        let saved = SavedUndo {
            hash: hash(text),
            history,
        };
        write_json(&self.undo_path(path), &saved);
    }

    /// Returns the undo history saved for the file at `path`, if it was saved for exactly `text`.
    pub fn load_undo(&self, path: &Path, text: &str) -> Option<History> {
        let saved: SavedUndo = read_json(&self.undo_path(path))?;
        (saved.hash == hash(text)).then_some(saved.history)
    }
}

#[cfg(test)]
/// Tests for saved state.
mod tests {
    use std::{env, fs, path::Path, process};

    use mog_core::{Document, Range, Transaction};

    use super::{Session, SessionFile, State};

    /// Makes an empty state folder for one test.
    fn state(name: &str) -> State {
        let dir = env::temp_dir().join(format!("mog-state-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        State::at(dir)
    }

    /// Sessions come back for the same project only.
    #[test]
    fn sessions_round_trip() {
        let state = state("session");
        let session = Session {
            files: vec![SessionFile {
                path: "/code/a.rs".into(),
                anchor: 1,
                head: 4,
                scroll_line: 2,
            }],
            active: 0,
            split: Some(0),
            explorer_open: true,
        };
        state.save_session(Path::new("/code"), &session);
        assert_eq!(state.load_session(Path::new("/code")), Some(session));
        assert_eq!(state.load_session(Path::new("/other")), None);
    }

    /// Undo history only comes back for the text it was saved with.
    #[test]
    fn undo_needs_the_same_text() {
        let state = state("undo");
        let mut document = Document::from_text("ab");
        document.apply(Transaction::insert(2, "c"), Range::point(3), false);
        let text = document.text().to_string();
        let path = Path::new("/code/a.txt");
        state.save_undo(path, &text, document.history());
        let history = state.load_undo(path, &text).expect("saved history");
        let mut reopened = Document::from_text(&text);
        reopened.restore_history(history);
        assert!(reopened.undo());
        assert_eq!(reopened.text().to_string(), "ab");
        assert!(state.load_undo(path, "abX").is_none());
    }

    /// Swap files from a running mog, this one, are not offered for recovery.
    #[test]
    fn running_swaps_are_left_alone() {
        let state = state("swap");
        let root = Path::new("/code");
        let file = state.write_swap(
            "/code/a.txt",
            Some(Path::new("/code/a.txt")),
            root,
            "x".into(),
        );
        assert!(file.exists());
        assert!(state.orphaned_swaps(root).is_empty());
    }
}
