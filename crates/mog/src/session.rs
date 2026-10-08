//! Remembering open files between runs, unsaved work after a crash and undo history.
//!
//! Everything lives as JSON under [`state_dir`], in `sessions`, `swap` and `undo` folders, with
//! file names hashed from the project or file they are for. Writes happen on a background thread
//! so a big buffer never stalls typing.

use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SendError, Sender},
    },
    thread,
};

use mog_config::state_dir;
use mog_core::{History, Rope};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System};

/// The most undo steps kept for one file.
const MAX_UNDO: usize = 500;

/// Files bigger than this many bytes do not get their undo history saved.
const MAX_UNDO_FILE: usize = 4 << 20;

/// Counts temp files so two writes in one process never share a name.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// The breakpoints of each file, lines counted from 0.
    #[serde(default)]
    pub breakpoints: BTreeMap<PathBuf, Vec<usize>>,
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
    /// When that process started, in seconds since the epoch, so a reused pid is not mistaken
    /// for it. `0` when unknown.
    #[serde(default)]
    pub started: u64,
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

/// Returns when the process `pid` started, or `None` if it is not running.
fn start_time(pid: u32) -> Option<u64> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(pid).map(Process::start_time)
}

/// Returns whether the mog that wrote `swap` is still running.
fn is_running(swap: &Swap) -> bool {
    start_time(swap.pid).is_some_and(|started| swap.started == 0 || started == swap.started)
}

/// Reads JSON from `path`, or `None` if it is missing or broken.
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Makes `options` create files only their owner can read, since they hold unsaved work.
#[cfg(unix)]
fn owner_only(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
}

/// Does nothing, since the state folder is already private to the user on Windows.
#[cfg(not(unix))]
fn owner_only(_options: &mut OpenOptions) {}

/// Creates `dir` and its parents, private to the owner where the system supports it.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Writes `data` to `path` so a crash or power loss leaves either the old file or the new one.
///
/// # Errors
///
/// Returns an error if the folder, the temp file or the rename fails.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // unique per process and write, so two mogs or two writes never share a temp file
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.{}.{count}.tmp", process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    owner_only(&mut options);
    let written = options.open(&temp).and_then(|mut file: File| {
        file.write_all(data)?;
        file.sync_all()
    });
    if let Err(err) = written.and_then(|()| fs::rename(&temp, path)) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    // the rename itself is only durable once the folder is synced
    #[cfg(unix)]
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Serializes `value` and writes it to `path` with [`write_atomic`].
///
/// # Errors
///
/// Returns an error if serializing or writing fails.
fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_vec(value).map_err(io::Error::other)?;
    write_atomic(path, &text)
}

/// A write for the writer thread to do.
type WriteJob = Box<dyn FnOnce() -> io::Result<()> + Send>;

/// Work for the writer thread.
enum Job {
    /// Runs a write, keeping its error.
    Write(WriteJob),
    /// Says when everything before it is done.
    Flush(Sender<()>),
}

/// A thread that does state writes in order, off the UI thread.
#[derive(Debug, Clone)]
struct Writer {
    /// Where jobs go, `None` if the thread could not start and writes happen inline.
    jobs: Option<Sender<Job>>,
    /// The last write error nobody has seen yet.
    error: Arc<Mutex<Option<String>>>,
}

impl Writer {
    /// Starts the writer thread.
    fn start() -> Self {
        let (jobs, queue) = mpsc::channel::<Job>();
        let error = Arc::new(Mutex::new(None));
        let errors = Arc::clone(&error);
        let spawned = thread::Builder::new()
            .name("mog-state".into())
            .spawn(move || {
                for job in queue {
                    match job {
                        Job::Write(write) => {
                            if let Err(err) = write() {
                                *errors.lock().unwrap_or_else(PoisonError::into_inner) =
                                    Some(err.to_string());
                            }
                        }
                        Job::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            });
        Self {
            jobs: spawned.is_ok().then_some(jobs),
            error,
        }
    }

    /// Runs `write` on the writer thread, or right here if there is none.
    fn run(&self, write: impl FnOnce() -> io::Result<()> + Send + 'static) {
        let mut write: WriteJob = Box::new(write);
        if let Some(jobs) = &self.jobs {
            match jobs.send(Job::Write(write)) {
                Ok(()) => return,
                Err(SendError(Job::Write(job))) => write = job,
                Err(SendError(Job::Flush(_))) => return,
            }
        }
        if let Err(err) = write() {
            *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(err.to_string());
        }
    }

    /// Waits for every write sent so far.
    fn flush(&self) {
        let Some(jobs) = &self.jobs else {
            return;
        };
        let (done, wait) = mpsc::channel();
        if jobs.send(Job::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }

    /// Returns the last write error since the last call.
    fn take_error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// Where sessions, swap files and undo history are kept.
#[derive(Debug, Clone)]
pub struct State {
    /// The state folder.
    dir: PathBuf,
    /// When this process started, written into swaps.
    started: u64,
    /// Does the writing.
    writer: Writer,
}

impl State {
    /// Returns the state in the usual folder, if the system has one.
    pub fn open() -> Option<Self> {
        state_dir().map(Self::at)
    }

    /// Returns the state kept in `dir`.
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            started: start_time(process::id()).unwrap_or_default(),
            writer: Writer::start(),
        }
    }

    /// Waits until every write so far is on disk.
    pub fn flush(&self) {
        self.writer.flush();
    }

    /// Returns why the last write failed, once, if one did.
    pub fn take_error(&self) -> Option<String> {
        self.writer.take_error()
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
        let path = self.session_path(root);
        let session = session.clone();
        self.writer.run(move || write_json(&path, &session));
    }

    /// Returns the file the recently run commands are kept in.
    fn recent_path(&self) -> PathBuf {
        self.dir.join("recent_commands.json")
    }

    /// Returns the commands last run from the palette, most recent first.
    pub fn load_recent_commands(&self) -> Vec<String> {
        read_json(&self.recent_path()).unwrap_or_default()
    }

    /// Saves the commands last run from the palette, most recent first.
    pub fn save_recent_commands(&self, commands: &[String]) {
        let (path, commands) = (self.recent_path(), commands.to_vec());
        self.writer.run(move || write_json(&path, &commands));
    }

    /// Returns the swap file for the document called `key`, a path or an untitled name.
    pub fn swap_path(&self, key: &str) -> PathBuf {
        let pid = process::id();
        self.dir
            .join("swap")
            .join(format!("{}-{pid}.json", hash(key)))
    }

    /// Writes unsaved `text` for the document called `key` and returns where it goes.
    ///
    /// Cloning a [`Rope`] is cheap, so the copy into a string happens on the writer thread.
    pub fn write_swap(&self, key: &str, path: Option<&Path>, root: &Path, text: Rope) -> PathBuf {
        let file = self.swap_path(key);
        let (path, root, started) = (path.map(ToOwned::to_owned), root.to_owned(), self.started);
        let target = file.clone();
        self.writer.run(move || {
            let swap = Swap {
                path,
                root,
                pid: process::id(),
                started,
                text: text.to_string(),
            };
            write_json(&target, &swap)
        });
        file
    }

    /// Removes the swap file `file`, after any write to it still queued.
    pub fn remove_swap(&self, file: PathBuf) {
        self.writer.run(move || match fs::remove_file(&file) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        });
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
            .filter(|(_, swap)| swap.root == root && !is_running(swap))
            .collect()
    }

    /// Returns the undo history file for the file at `path`.
    fn undo_path(&self, path: &Path) -> PathBuf {
        let key = hash(&path.to_string_lossy());
        self.dir.join("undo").join(format!("{key}.json"))
    }

    /// Saves `history` for the file at `path` whose text is now `text`.
    pub fn save_undo(&self, path: &Path, text: &Rope, history: &History) {
        if history.is_empty() || text.len_bytes() > MAX_UNDO_FILE {
            return;
        }
        let mut history = history.clone();
        let (file, text) = (self.undo_path(path), text.clone());
        self.writer.run(move || {
            history.truncate(MAX_UNDO);
            let saved = SavedUndo {
                hash: hash(&text.to_string()),
                history,
            };
            write_json(&file, &saved)
        });
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
    use std::{collections::BTreeMap, env, fs, path::Path, process};

    use mog_core::{Document, Range, Rope, Transaction};

    use super::{Session, SessionFile, State, Swap, is_running, write_atomic};

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
            breakpoints: BTreeMap::from([("/code/a.rs".into(), vec![3])]),
        };
        state.save_session(Path::new("/code"), &session);
        state.flush();
        assert_eq!(state.load_session(Path::new("/code")), Some(session));
        assert_eq!(state.load_session(Path::new("/other")), None);
        assert_eq!(state.take_error(), None);
    }

    /// The commands run from the palette come back in the same order.
    #[test]
    fn recent_commands_round_trip() {
        let state = state("recent");
        assert!(state.load_recent_commands().is_empty());
        let recent = vec!["save".to_owned(), "undo".to_owned()];
        state.save_recent_commands(&recent);
        state.flush();
        assert_eq!(state.load_recent_commands(), recent);
    }

    /// Undo history only comes back for the text it was saved with.
    #[test]
    fn undo_needs_the_same_text() {
        let state = state("undo");
        let mut document = Document::from_text("ab");
        document.apply(Transaction::insert(2, "c"), Range::point(3), false);
        let text = document.text().to_string();
        let path = Path::new("/code/a.txt");
        state.save_undo(path, document.text(), document.history());
        state.flush();
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
            Rope::from_str("x"),
        );
        state.flush();
        assert!(file.exists());
        assert!(state.orphaned_swaps(root).is_empty());
        state.remove_swap(file.clone());
        state.flush();
        assert!(!file.exists());
    }

    /// A swap from a process that reused a running pid counts as left behind.
    #[test]
    fn reused_pids_are_not_running() {
        let swap = |started| Swap {
            path: None,
            root: "/code".into(),
            pid: process::id(),
            started,
            text: String::new(),
        };
        assert!(is_running(&swap(0)));
        assert!(!is_running(&swap(1)));
    }

    /// Atomic writes replace the whole file and leave no temp files behind.
    #[test]
    fn writes_atomically() {
        let dir = env::temp_dir().join(format!("mog-atomic-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("a.json");
        write_atomic(&file, b"one").expect("first");
        write_atomic(&file, b"two").expect("second");
        assert_eq!(fs::read(&file).expect("read"), b"two");
        assert_eq!(fs::read_dir(&dir).expect("dir").count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
