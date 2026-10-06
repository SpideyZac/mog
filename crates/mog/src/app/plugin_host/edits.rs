//! Workspace edits from plugins: checking every part against the text as it is, then applying
//! all of them or none.

use std::{
    fs::{self, File},
    io::{self, BufWriter, Write as _},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use mog_core::{Change, Command, Document, Range, Rope, Transaction};
use mog_plugin::FileEdit;

use super::to_changes;
use crate::app::App;

/// Counts temp files so two edits never share one.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Which file a part of a workspace edit is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    /// An open document, by index.
    Open(usize),
    /// A file that is not open.
    Closed(PathBuf),
}

/// Every part of a workspace edit for one file, merged.
struct Group {
    /// The file.
    key: Key,
    /// The file name for messages.
    name: String,
    /// The changes of every part, in offsets of the text before the edit.
    changes: Vec<Change>,
}

/// A file that is not open, checked and ready to write.
struct Closed {
    /// Where it is.
    path: PathBuf,
    /// Its text before the edit, or `None` when the edit creates it.
    before: Option<Rope>,
    /// Its text after the edit.
    after: Rope,
}

/// A closed file written to a temp file next to it, waiting to replace it.
struct Staged {
    /// The file.
    closed: Closed,
    /// The temp file holding the new text.
    temp: PathBuf,
}

/// Writes `text` to a new temp file next to `path`, with the permissions of `path` if it exists.
///
/// # Errors
///
/// Returns an error if the temp file cannot be written.
fn write_temp(path: &Path, text: &Rope) -> io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.mog-edit.{}.{count}.tmp", process::id()));
    let written = File::create_new(&temp).and_then(|file| {
        let mut writer = BufWriter::new(file);
        text.write_to(&mut writer)?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()?;
        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&temp, metadata.permissions())?;
        }
        Ok(())
    });
    match written {
        Ok(()) => Ok(temp),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// Puts back what `closed` held before the edit, deleting it if the edit created it.
fn restore(closed: &Closed) {
    match &closed.before {
        Some(text) => {
            if let Ok(temp) = write_temp(&closed.path, text)
                && fs::rename(&temp, &closed.path).is_err()
            {
                let _ = fs::remove_file(temp);
            }
        }
        None => {
            let _ = fs::remove_file(&closed.path);
        }
    }
}

/// Writes every closed file, all of them or none: each goes to a temp file first, and only once
/// all are written do they replace the files. A replace that fails puts back the ones before it.
///
/// Returns the files written, so they can be put back if a later step fails.
///
/// # Errors
///
/// Returns why a file could not be written, after undoing the ones that were.
fn write_closed(files: Vec<Closed>) -> Result<Vec<Closed>, String> {
    let mut staged = Vec::with_capacity(files.len());
    for closed in files {
        match write_temp(&closed.path, &closed.after) {
            Ok(temp) => staged.push(Staged { closed, temp }),
            Err(err) => {
                for staged in staged {
                    let _ = fs::remove_file(staged.temp);
                }
                return Err(format!("could not save {}: {err}", closed.path.display()));
            }
        }
    }
    let mut written: Vec<Closed> = Vec::with_capacity(staged.len());
    let mut staged = staged.into_iter();
    while let Some(next) = staged.next() {
        if let Err(err) = fs::rename(&next.temp, &next.closed.path) {
            let _ = fs::remove_file(&next.temp);
            for rest in staged {
                let _ = fs::remove_file(rest.temp);
            }
            for done in &written {
                restore(done);
            }
            return Err(format!(
                "could not save {}: {err}",
                next.closed.path.display()
            ));
        }
        written.push(next.closed);
    }
    Ok(written)
}

impl App {
    /// Applies changes to one or more files, all of them or none.
    ///
    /// Parts for the same file are merged, since each is in offsets of the text before the
    /// edit. Open files change in the editor as one undo step each. Files that are not open are
    /// changed and saved on disk.
    ///
    /// # Errors
    ///
    /// Returns why a part does not fit, like a stale version or overlapping changes, or why a
    /// file could not be saved. Nothing is changed either way.
    pub(super) fn apply_workspace_edit(&mut self, edits: Vec<FileEdit>) -> Result<(), String> {
        let mut groups: Vec<Group> = Vec::new();
        for edit in edits {
            let path = self.resolve_path(edit.path.as_deref());
            let name = path.as_ref().map_or_else(
                || "the focused file".to_owned(),
                |path| path.display().to_string(),
            );
            let key = match self.find_document(path.as_deref()) {
                Some(index) => {
                    let document = &self.editor.documents()[index];
                    if let Some(version) = edit.version
                        && version != document.version()
                    {
                        return Err(format!(
                            "{name} changed since version {version}, it is at {} now",
                            document.version()
                        ));
                    }
                    Key::Open(index)
                }
                None => Key::Closed(path.ok_or("no file is open")?),
            };
            let changes = to_changes(edit.changes);
            match groups.iter_mut().find(|group| group.key == key) {
                Some(group) => group.changes.extend(changes),
                None => groups.push(Group { key, name, changes }),
            }
        }
        let mut open = Vec::new();
        let mut closed = Vec::new();
        for group in groups {
            let tx = Transaction::try_new(group.changes.clone())
                .map_err(|err| format!("{}: {err}", group.name))?;
            match group.key {
                Key::Open(index) => {
                    tx.check_bounds(self.editor.documents()[index].text().len_chars())
                        .map_err(|err| format!("{}: {err}", group.name))?;
                    open.push((index, group.changes));
                }
                Key::Closed(path) => {
                    let existed = path.exists();
                    let mut document = Document::open(&path)
                        .map_err(|err| format!("could not open {}: {err}", group.name))?;
                    tx.check_bounds(document.text().len_chars())
                        .map_err(|err| format!("{}: {err}", group.name))?;
                    let before = existed.then(|| document.text().clone());
                    document.apply(tx, Range::point(0), false);
                    closed.push(Closed {
                        path: document.path().map_or(path, ToOwned::to_owned),
                        before,
                        after: document.text().clone(),
                    });
                }
            }
        }
        // files on disk go first, they are the part that can still fail
        let written = write_closed(closed)?;
        let focused = self.editor.active();
        let mut applied = Vec::new();
        for (index, changes) in open {
            self.editor.focus(index);
            if let Err(err) = self.editor.try_apply_changes(changes) {
                for index in applied {
                    self.editor.focus(index);
                    self.execute_command(Command::Undo);
                }
                for closed in &written {
                    restore(closed);
                }
                self.editor.focus(focused);
                return Err(err.to_string());
            }
            applied.push(index);
        }
        self.editor.focus(focused);
        Ok(())
    }
}
