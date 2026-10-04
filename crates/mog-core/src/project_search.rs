//! Finding and replacing text across every file in a project.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use ropey::Rope;

use crate::{file_tree::walk_files, search::find_all, transaction::Change};

/// The most files a project search looks at.
const MAX_FILES: usize = 20_000;

/// Files bigger than this many bytes are skipped, they are probably not code.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// The longest a match preview line gets, in chars.
const MAX_PREVIEW: usize = 200;

/// One place the query was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMatch {
    /// The file it is in.
    pub path: PathBuf,
    /// The line, from 0.
    pub line: usize,
    /// The char column the match starts at, from 0.
    pub column: usize,
    /// The column the match starts at in the preview, where tabs are wider.
    pub preview_column: usize,
    /// How many chars the match covers.
    pub len: usize,
    /// The line the match is on, with tabs as spaces and cut short if long.
    pub preview: String,
}

/// What a project search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectResults {
    /// The matches in file order, at most the limit asked for.
    pub matches: Vec<ProjectMatch>,
    /// How many files had a match.
    pub files: usize,
    /// Whether more matches were found than kept.
    pub truncated: bool,
}

/// Returns every match of `query` in `text`, which is the contents of `path`.
pub fn search_text(
    path: &Path,
    text: &str,
    query: &str,
    case_sensitive: bool,
) -> Vec<ProjectMatch> {
    let mut found = Vec::new();
    for (line, content) in text.lines().enumerate() {
        let rope = Rope::from_str(content);
        for (from, to) in find_all(&rope, query, case_sensitive) {
            let preview: String = content
                .replace('\t', "    ")
                .chars()
                .take(MAX_PREVIEW)
                .collect();
            let tabs = content.chars().take(from).filter(|&ch| ch == '\t').count();
            found.push(ProjectMatch {
                path: path.to_owned(),
                line,
                column: from,
                preview_column: from + tabs * 3,
                len: to - from,
                preview,
            });
        }
    }
    found
}

/// Reads the text of `path`, or `None` for big, unreadable or binary files.
fn read_text(path: &Path) -> Option<String> {
    let size = fs::metadata(path).ok()?.len();
    if size > MAX_FILE_BYTES {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    (!text.contains('\0')).then_some(text)
}

/// Searches every file under `root` for `query`, keeping at most `limit` matches.
///
/// Files in `open` are searched as given instead of read from disk, so unsaved changes count.
/// `cancelled` is asked between files so a newer search can stop this one.
pub fn search_project(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    open: &HashMap<PathBuf, String>,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
) -> ProjectResults {
    let mut results = ProjectResults::default();
    if query.is_empty() {
        return results;
    }
    for path in walk_files(root, MAX_FILES) {
        if cancelled() {
            break;
        }
        let text = match open.get(&path) {
            Some(text) => text.clone(),
            None => match read_text(&path) {
                Some(text) => text,
                None => continue,
            },
        };
        let found = search_text(&path, &text, query, case_sensitive);
        if found.is_empty() {
            continue;
        }
        results.files += 1;
        let room = limit.saturating_sub(results.matches.len());
        if found.len() > room {
            results.truncated = true;
        }
        results.matches.extend(found.into_iter().take(room));
    }
    results
}

/// Returns the changes that replace every match of `query` in `text` with `replacement`.
pub fn replace_changes(
    text: &Rope,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> Vec<Change> {
    find_all(text, query, case_sensitive)
        .into_iter()
        .map(|(start, end)| Change {
            start,
            end,
            text: replacement.to_owned(),
        })
        .collect()
}

/// Replaces every match of `query` in the file at `path` and saves it.
///
/// Returns how many matches were replaced.
///
/// # Errors
///
/// Returns an error if the file cannot be read or written.
pub fn replace_in_file(
    path: &Path,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> io::Result<usize> {
    let text = fs::read_to_string(path)?;
    let mut rope = Rope::from_str(&text);
    let changes = replace_changes(&rope, query, replacement, case_sensitive);
    // back to front so earlier offsets stay right
    for change in changes.iter().rev() {
        rope.remove(change.start..change.end);
        rope.insert(change.start, &change.text);
    }
    if !changes.is_empty() {
        fs::write(path, rope.to_string())?;
    }
    Ok(changes.len())
}

#[cfg(test)]
/// Tests for project search.
mod tests {
    use std::{collections::HashMap, env, fs, path::PathBuf, process};

    use ropey::Rope;

    use super::{replace_changes, replace_in_file, search_project, search_text};

    /// Creates an empty scratch folder unique to `name`.
    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mog-project-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Matches know their line, column and length.
    #[test]
    fn finds_lines_and_columns() {
        let found = search_text(
            "a.rs".as_ref(),
            "fn mog() {}\n\tlet x = MOG;\n",
            "mog",
            false,
        );
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].line, found[0].column, found[0].len), (0, 3, 3));
        assert_eq!((found[1].line, found[1].column), (1, 9));
        assert_eq!(found[1].preview_column, 12);
        assert_eq!(found[1].preview, "    let x = MOG;");
        assert_eq!(search_text("a".as_ref(), "Mog", "mog", true).len(), 0);
    }

    /// Searching a folder reads files, prefers open text and respects the limit.
    #[test]
    fn searches_a_folder() {
        let dir = scratch("search");
        fs::write(dir.join("a.txt"), "mog\nmog mog\n").expect("write");
        fs::write(dir.join("b.txt"), "nothing\n").expect("write");
        fs::write(dir.join("c.bin"), "mog\0").expect("write");
        let mut open = HashMap::new();
        open.insert(dir.join("b.txt"), "unsaved mog\n".to_owned());
        let results = search_project(&dir, "mog", false, &open, 100, &|| false);
        assert_eq!(results.files, 2);
        assert_eq!(results.matches.len(), 4);
        let limited = search_project(&dir, "mog", false, &open, 2, &|| false);
        assert_eq!(limited.matches.len(), 2);
        assert!(limited.truncated);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Replacing changes every match and nothing else.
    #[test]
    fn replaces_matches() {
        let rope = Rope::from_str("Mog and mog");
        let changes = replace_changes(&rope, "mog", "vim", false);
        assert_eq!(changes.len(), 2);
        let dir = scratch("replace");
        let path = dir.join("a.txt");
        fs::write(&path, "Mog and mog\n").expect("write");
        assert_eq!(
            replace_in_file(&path, "mog", "emacs", true).expect("replace"),
            1
        );
        assert_eq!(fs::read_to_string(&path).expect("read"), "Mog and emacs\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
