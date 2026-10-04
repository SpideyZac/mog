//! Finding and replacing text across every file in a project.

use std::{
    collections::HashMap,
    fs, io, iter,
    path::{Path, PathBuf},
};

use ropey::Rope;

use crate::{
    file_tree::walk_files,
    search::{Matcher, SearchOptions, replace_changes},
};

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

/// Returns every match of `matcher` in `text`, which is the contents of `path`.
pub fn search_text(path: &Path, text: &str, matcher: &Matcher) -> Vec<ProjectMatch> {
    let ranges = matcher.find_in(text);
    if ranges.is_empty() {
        return Vec::new();
    }
    let line_starts: Vec<usize> = iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    ranges
        .into_iter()
        .map(|(start, end)| {
            let line = line_starts.partition_point(|&at| at <= start) - 1;
            let line_start = line_starts[line];
            let content = text[line_start..]
                .split('\n')
                .next()
                .unwrap_or("")
                .trim_end_matches('\r');
            let offset = (start - line_start).min(content.len());
            make_match(
                path,
                line,
                content,
                offset,
                text[start..end].chars().count(),
            )
        })
        .collect()
}

/// Builds the match of `len` chars at byte `start` of the line `content`.
fn make_match(path: &Path, line: usize, content: &str, start: usize, len: usize) -> ProjectMatch {
    let before = &content[..start];
    let column = before.chars().count();
    let tabs = before.chars().filter(|&ch| ch == '\t').count();
    ProjectMatch {
        path: path.to_owned(),
        line,
        column,
        preview_column: column + tabs * 3,
        len,
        preview: content
            .replace('\t', "    ")
            .chars()
            .take(MAX_PREVIEW)
            .collect(),
    }
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
///
/// # Errors
///
/// Returns the reason when the query is a broken regular expression.
pub fn search_project(
    root: &Path,
    query: &str,
    options: SearchOptions,
    open: &HashMap<PathBuf, String>,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<ProjectResults, String> {
    let mut results = ProjectResults::default();
    if query.is_empty() {
        return Ok(results);
    }
    let matcher = Matcher::new(query, options)?;
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
        let found = search_text(&path, &text, &matcher);
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
    Ok(results)
}

/// Replaces every match of `matcher` in the file at `path` and saves it.
///
/// Returns how many matches were replaced.
///
/// # Errors
///
/// Returns an error if the file cannot be read or written.
pub fn replace_in_file(path: &Path, matcher: &Matcher, replacement: &str) -> io::Result<usize> {
    let text = fs::read_to_string(path)?;
    let mut rope = Rope::from_str(&text);
    let changes = replace_changes(&rope, matcher, replacement);
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
    use std::{
        collections::HashMap,
        env, fs,
        path::{Path, PathBuf},
        process,
    };

    use super::{replace_in_file, search_project, search_text};
    use crate::search::{Matcher, SearchOptions};

    /// Creates an empty scratch folder unique to `name`.
    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mog-project-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Builds a matcher for `query`.
    fn matcher(query: &str, options: SearchOptions) -> Matcher {
        Matcher::new(query, options).expect("valid query")
    }

    /// Matches know their line, column and length, past tabs, accents and windows line ends.
    #[test]
    fn finds_lines_and_columns() {
        let plain = SearchOptions::default();
        let text = "\tl\u{e9}t M\u{f6}g = mog;\r\n\tlet x = MOG;\n";
        let found = search_text(Path::new("a"), text, &matcher("mog", plain));
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].line, found[0].column, found[0].len), (0, 11, 3));
        assert_eq!((found[1].line, found[1].column), (1, 9));
        assert_eq!(found[1].preview_column, 12);
        assert_eq!(found[1].preview, "    let x = MOG;");
        let accented = search_text(Path::new("a"), text, &matcher("m\u{f6}g", plain));
        assert_eq!(accented.len(), 1);
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
        let plain = SearchOptions::default();
        let results = search_project(&dir, "mog", plain, &open, 100, &|| false).expect("valid");
        assert_eq!(results.files, 2);
        assert_eq!(results.matches.len(), 4);
        let limited = search_project(&dir, "mog", plain, &open, 2, &|| false).expect("valid");
        assert_eq!(limited.matches.len(), 2);
        assert!(limited.truncated);
        let regex = SearchOptions {
            regex: true,
            ..plain
        };
        assert!(search_project(&dir, "(", regex, &open, 2, &|| false).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Replacing a file changes every match and nothing else.
    #[test]
    fn replaces_in_files() {
        let dir = scratch("replace");
        let path = dir.join("a.txt");
        fs::write(&path, "Mog and mog\n").expect("write");
        let case = SearchOptions {
            case_sensitive: true,
            ..SearchOptions::default()
        };
        let replaced = replace_in_file(&path, &matcher("mog", case), "emacs").expect("replace");
        assert_eq!(replaced, 1);
        assert_eq!(fs::read_to_string(&path).expect("read"), "Mog and emacs\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
