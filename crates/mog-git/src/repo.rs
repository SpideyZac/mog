//! Asking `git` about a repository.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

/// The name `git blame` gives lines that are not committed yet.
const NOT_COMMITTED: &str = "Not Committed Yet";

/// The state of a file in the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileStatus {
    /// Changed since the last commit.
    Modified,
    /// New and staged.
    Added,
    /// New and not known to git.
    Untracked,
    /// Removed.
    Deleted,
    /// Moved or renamed.
    Renamed,
    /// Has merge conflicts.
    Conflicted,
}

impl FileStatus {
    /// Parses the two letter code from `git status --porcelain`.
    fn from_code(code: &str) -> Option<Self> {
        let mut chars = code.chars();
        let (x, y) = (chars.next()?, chars.next()?);
        Some(match (x, y) {
            ('?', '?') => Self::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => Self::Conflicted,
            ('R', _) => Self::Renamed,
            ('A', _) => Self::Added,
            ('D', _) | (_, 'D') => Self::Deleted,
            (' ', ' ') => return None,
            _ => Self::Modified,
        })
    }
}

/// Who last changed a line and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blame {
    /// The author name, or `None` if the line is not committed yet.
    pub author: Option<String>,
    /// The commit time in seconds since the Unix epoch.
    pub time: u64,
    /// The first line of the commit message.
    pub summary: String,
}

impl Blame {
    /// Formats the blame like `zac, 3 days ago - fix the thing`.
    pub fn describe(&self, now: u64) -> String {
        match &self.author {
            None => "you, not committed yet".into(),
            Some(author) => format!(
                "{author}, {} - {}",
                relative_time(now.saturating_sub(self.time)),
                self.summary
            ),
        }
    }
}

/// Describes a number of seconds ago in rough human terms.
pub fn relative_time(seconds: u64) -> String {
    let units = [
        (365 * 24 * 3600, "year"),
        (30 * 24 * 3600, "month"),
        (7 * 24 * 3600, "week"),
        (24 * 3600, "day"),
        (3600, "hour"),
        (60, "minute"),
    ];
    for (size, name) in units {
        let count = seconds / size;
        if count > 0 {
            let plural = if count == 1 { "" } else { "s" };
            return format!("{count} {name}{plural} ago");
        }
    }
    "just now".into()
}

/// Returns the current time in seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

/// Rebuilds a path from its components so `/` and `\` separators compare equal.
fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// A git repository on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The top folder of the working tree.
    root: PathBuf,
}

impl Repo {
    /// Finds the repository containing `path`, or `None` if it is not in one or git is missing.
    pub fn discover(path: &Path) -> Option<Self> {
        let dir = if path.is_dir() { path } else { path.parent()? };
        let output = run(dir, &["rev-parse", "--show-toplevel"], None)?;
        let root = output.trim();
        (!root.is_empty()).then(|| Self {
            root: normalize(Path::new(root)),
        })
    }

    /// Returns the top folder of the working tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the checked out branch, or the short commit id when detached.
    pub fn branch(&self) -> Option<String> {
        let name = self.git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        let name = name.trim();
        if name == "HEAD" {
            return self
                .git(&["rev-parse", "--short", "HEAD"])
                .map(|id| id.trim().to_owned());
        }
        (!name.is_empty()).then(|| name.to_owned())
    }

    /// Returns how many commits the branch is ahead and behind its upstream.
    pub fn ahead_behind(&self) -> Option<(usize, usize)> {
        let counts = self.git(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])?;
        let mut parts = counts.split_whitespace().map(str::parse);
        match (parts.next(), parts.next()) {
            (Some(Ok(ahead)), Some(Ok(behind))) => Some((ahead, behind)),
            _ => None,
        }
    }

    /// Returns the status of every changed file by absolute path.
    pub fn status(&self) -> HashMap<PathBuf, FileStatus> {
        let Some(output) = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        else {
            return HashMap::new();
        };
        let mut statuses = HashMap::new();
        let mut entries = output.split('\0');
        while let Some(entry) = entries.next() {
            if entry.len() < 4 {
                continue;
            }
            let (code, path) = entry.split_at(3);
            let Some(status) = FileStatus::from_code(code) else {
                continue;
            };
            if status == FileStatus::Renamed {
                // renames are followed by the old path, which is not interesting
                entries.next();
            }
            statuses.insert(normalize(&self.root.join(path)), status);
        }
        statuses
    }

    /// Returns the text of `path` in the last commit, or `None` if it is not committed.
    pub fn head_text(&self, path: &Path) -> Option<String> {
        let relative = self.relative(path)?;
        self.git(&["show", &format!("HEAD:{relative}")])
    }

    /// Returns who last changed `line` (from 0) of `path`, given its current `contents`.
    pub fn blame(&self, path: &Path, line: usize, contents: &str) -> Option<Blame> {
        let relative = self.relative(path)?;
        let range = format!("{0},{0}", line + 1);
        let output = run(
            &self.root,
            &[
                "blame",
                "--porcelain",
                "-L",
                &range,
                "--contents",
                "-",
                "--",
                &relative,
            ],
            Some(contents),
        )?;
        parse_blame(&output)
    }

    /// Returns `path` relative to the root with `/` separators.
    fn relative(&self, path: &Path) -> Option<String> {
        let relative = normalize(path).strip_prefix(&self.root).ok()?.to_owned();
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(parts.join("/"))
    }

    /// Runs git in the root with `args`.
    fn git(&self, args: &[&str]) -> Option<String> {
        run(&self.root, args, None)
    }
}

/// Parses the output of `git blame --porcelain` for one line.
fn parse_blame(output: &str) -> Option<Blame> {
    let mut author = None;
    let mut time = 0;
    let mut summary = String::new();
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("author ") {
            author = Some(name.to_owned());
        } else if let Some(seconds) = line.strip_prefix("author-time ") {
            time = seconds.parse().unwrap_or(0);
        } else if let Some(text) = line.strip_prefix("summary ") {
            summary = text.to_owned();
        }
    }
    let author = author?;
    Some(Blame {
        author: (author != NOT_COMMITTED).then_some(author),
        time,
        summary,
    })
}

/// Runs git in `dir` with `args`, feeding it `input`, and returns its output if it succeeded.
fn run(dir: &Path, args: &[&str], input: Option<&str>) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).ok()?;
    }
    let output = child.wait_with_output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
/// Tests for repository helpers.
mod tests {
    use super::{Blame, FileStatus, parse_blame, relative_time};

    /// Porcelain codes map to statuses.
    #[test]
    fn status_codes() {
        assert_eq!(FileStatus::from_code("?? "), Some(FileStatus::Untracked));
        assert_eq!(FileStatus::from_code(" M "), Some(FileStatus::Modified));
        assert_eq!(FileStatus::from_code("A  "), Some(FileStatus::Added));
        assert_eq!(FileStatus::from_code("UU "), Some(FileStatus::Conflicted));
        assert_eq!(FileStatus::from_code(" D "), Some(FileStatus::Deleted));
    }

    /// Blame output is parsed and uncommitted lines have no author.
    #[test]
    fn parses_blame() {
        let output = "abc 1 1 1\nauthor zac\nauthor-time 100\nsummary fix it\n\tcode\n";
        let blame = parse_blame(output).expect("blame");
        assert_eq!(blame.describe(100 + 3 * 86_400), "zac, 3 days ago - fix it");
        let fresh = "000 1 1 1\nauthor Not Committed Yet\nauthor-time 5\nsummary x\n";
        assert_eq!(
            parse_blame(fresh),
            Some(Blame {
                author: None,
                time: 5,
                summary: "x".into()
            })
        );
    }

    /// Small gaps read as just now and larger ones pick the biggest unit.
    #[test]
    fn relative_times() {
        assert_eq!(relative_time(5), "just now");
        assert_eq!(relative_time(3600), "1 hour ago");
        assert_eq!(relative_time(2 * 365 * 86_400), "2 years ago");
    }
}
