//! Asking `git` about a repository.

use std::{
    collections::HashMap,
    fs,
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

/// A changed file, with what is staged and what is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The file, as an absolute path.
    pub path: PathBuf,
    /// The change staged for the next commit, if any.
    pub staged: Option<FileStatus>,
    /// The change in the working tree that is not staged, if any.
    pub unstaged: Option<FileStatus>,
}

/// Returns what one letter of a `git status --porcelain` code means.
fn status_letter(letter: char) -> Option<FileStatus> {
    Some(match letter {
        'M' | 'T' => FileStatus::Modified,
        'A' => FileStatus::Added,
        'D' => FileStatus::Deleted,
        'R' | 'C' => FileStatus::Renamed,
        'U' => FileStatus::Conflicted,
        '?' => FileStatus::Untracked,
        _ => return None,
    })
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

/// Resolves symlinks in `path`, going through its parent when the file does not exist yet.
fn resolve(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok().or_else(|| {
        Some(
            fs::canonicalize(path.parent()?)
                .ok()?
                .join(path.file_name()?),
        )
    })
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

    /// Returns every changed file with its staged and unstaged parts, sorted by path.
    pub fn changes(&self) -> Vec<FileChange> {
        let Some(output) = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        let mut entries = output.split('\0');
        while let Some(entry) = entries.next() {
            if entry.len() < 4 {
                continue;
            }
            let (code, path) = entry.split_at(3);
            let mut letters = code.chars();
            let (x, y) = (letters.next().unwrap_or(' '), letters.next().unwrap_or(' '));
            if matches!(x, 'R' | 'C') {
                entries.next();
            }
            let conflicted = FileStatus::from_code(code) == Some(FileStatus::Conflicted);
            let (staged, unstaged) = if conflicted {
                (None, Some(FileStatus::Conflicted))
            } else if x == '?' {
                (None, Some(FileStatus::Untracked))
            } else {
                (status_letter(x), status_letter(y))
            };
            changes.push(FileChange {
                path: normalize(&self.root.join(path)),
                staged,
                unstaged,
            });
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        changes
    }

    /// Returns the text of `path` staged in the index, or `None` if it is not in the index.
    pub fn index_text(&self, path: &Path) -> Option<String> {
        let relative = self.relative(path)?;
        self.git(&["show", &format!(":{relative}")])
    }

    /// Returns the diff of `path` as `git diff` prints it, of what is staged if `staged` is set.
    ///
    /// A file git does not know yet comes back as all added.
    pub fn diff(&self, path: &Path, staged: bool) -> String {
        let Some(relative) = self.relative(path) else {
            return String::new();
        };
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        if staged {
            args.push("--cached");
        }
        args.extend(["--", &relative]);
        let diff = self.git(&args).unwrap_or_default();
        if !diff.is_empty() || staged {
            return diff;
        }
        // untracked files have no diff, so show them as all new
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut out = format!(
                    "new file {relative}\n@@ -0,0 +1,{} @@\n",
                    text.lines().count()
                );
                for line in text.lines() {
                    out.push('+');
                    out.push_str(line);
                    out.push('\n');
                }
                out
            }
            Err(_) => String::new(),
        }
    }

    /// Stages all of `path`.
    ///
    /// # Errors
    ///
    /// Returns what git said if it failed.
    pub fn stage(&self, path: &Path) -> Result<(), String> {
        let relative = self
            .relative(path)
            .ok_or("the file is outside the repository")?;
        self.git_checked(&["add", "--", &relative], None).map(drop)
    }

    /// Stages every change in the working tree.
    ///
    /// # Errors
    ///
    /// Returns what git said if it failed.
    pub fn stage_all(&self) -> Result<(), String> {
        self.git_checked(&["add", "--all"], None).map(drop)
    }

    /// Takes all of `path` out of the next commit, keeping the changes in the working tree.
    ///
    /// # Errors
    ///
    /// Returns what git said if it failed.
    pub fn unstage(&self, path: &Path) -> Result<(), String> {
        let relative = self
            .relative(path)
            .ok_or("the file is outside the repository")?;
        if self.git(&["rev-parse", "--verify", "-q", "HEAD"]).is_some() {
            self.git_checked(&["reset", "-q", "HEAD", "--", &relative], None)
        } else {
            // before the first commit there is no HEAD to reset to
            self.git_checked(&["rm", "--cached", "-q", "--", &relative], None)
        }
        .map(drop)
    }

    /// Puts `text` in the index as the staged content of `path`, without touching the file.
    ///
    /// This is how single hunks are staged and unstaged.
    ///
    /// # Errors
    ///
    /// Returns what git said if it failed.
    pub fn set_index_text(&self, path: &Path, text: &str) -> Result<(), String> {
        let relative = self
            .relative(path)
            .ok_or("the file is outside the repository")?;
        let id = self.git_checked(&["hash-object", "-w", "--stdin"], Some(text))?;
        let mode = self
            .git(&["ls-files", "-s", "--", &relative])
            .and_then(|line| line.split_whitespace().next().map(str::to_owned))
            .unwrap_or_else(|| "100644".into());
        let info = format!("{mode},{},{relative}", id.trim());
        self.git_checked(&["update-index", "--add", "--cacheinfo", &info], None)
            .map(drop)
    }

    /// Commits what is staged with `message` and returns the short id of the new commit.
    ///
    /// # Errors
    ///
    /// Returns what git said if it failed, like when nothing is staged.
    pub fn commit(&self, message: &str) -> Result<String, String> {
        self.git_checked(&["commit", "-q", "-F", "-"], Some(message))?;
        Ok(self
            .git(&["rev-parse", "--short", "HEAD"])
            .unwrap_or_default()
            .trim()
            .to_owned())
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
        let path = normalize(path);
        let relative = match path.strip_prefix(&self.root) {
            Ok(relative) => relative.to_owned(),
            // git hands back the real root, so a path through a symlink like macOS /var misses
            Err(_) => resolve(&path)?
                .strip_prefix(fs::canonicalize(&self.root).ok()?)
                .ok()?
                .to_owned(),
        };
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

    /// Runs git in the root with `args` and `input`, returning what it printed or what it
    /// complained about.
    fn git_checked(&self, args: &[&str], input: Option<&str>) -> Result<String, String> {
        let mut child = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("could not run git: {err}"))?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin
                .write_all(text.as_bytes())
                .map_err(|err| err.to_string())?;
        }
        let output = child.wait_with_output().map_err(|err| err.to_string())?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("git failed")
            .to_owned();
        Err(message)
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
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process,
    };

    use super::{Blame, FileStatus, Repo, parse_blame, relative_time, run};
    use crate::diff::{apply_hunk, hunks};

    /// Makes a fresh repository with one committed file, or `None` without git.
    fn temp_repo(name: &str) -> Option<(Repo, PathBuf)> {
        let dir = env::temp_dir().join(format!("mog-git-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).ok()?;
        run(&dir, &["init", "-q"], None)?;
        run(&dir, &["config", "user.email", "mog@example.com"], None)?;
        run(&dir, &["config", "user.name", "mog"], None)?;
        run(&dir, &["config", "core.autocrlf", "false"], None)?;
        let file = dir.join("notes.txt");
        fs::write(&file, "a\nb\nc\nd\ne\n").ok()?;
        run(&dir, &["add", "."], None)?;
        run(&dir, &["commit", "-q", "-m", "first"], None)?;
        Some((Repo::discover(&dir)?, file))
    }

    /// One hunk can be staged and committed while the other stays in the working tree.
    #[test]
    fn stages_one_hunk_and_commits() {
        let Some((repo, file)) = temp_repo("hunk") else {
            return;
        };
        let changed = "a\nB\nc\nd\nE\n";
        fs::write(&file, changed).expect("write");
        let index = repo.index_text(&file).expect("in the index");
        let found = hunks(&index, changed);
        assert_eq!(found.len(), 2);
        repo.set_index_text(&file, &apply_hunk(&index, changed, &found[0]))
            .expect("staged");
        let changes = repo.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].staged, Some(FileStatus::Modified));
        assert_eq!(changes[0].unstaged, Some(FileStatus::Modified));
        assert!(repo.diff(&file, true).contains("+B"));
        assert!(!repo.diff(&file, true).contains("+E"));
        let id = repo.commit("stage one").expect("committed");
        assert!(!id.is_empty());
        assert_eq!(repo.head_text(&file).as_deref(), Some("a\nB\nc\nd\ne\n"));
        repo.stage(&file).expect("staged");
        repo.unstage(&file).expect("unstaged");
        assert_eq!(repo.changes()[0].staged, None);
        assert!(repo.commit("nothing").is_err());
        let _ = fs::remove_dir_all(Path::new(repo.root()));
    }

    /// A file reached through a symlinked folder still maps into the repository.
    #[cfg(unix)]
    #[test]
    fn finds_files_through_a_symlink() {
        let Some((repo, file)) = temp_repo("link") else {
            return;
        };
        let link = env::temp_dir().join(format!("mog-git-link-alias-{}", process::id()));
        let _ = fs::remove_file(&link);
        symlink(repo.root(), &link).expect("symlink");
        let aliased = link.join(file.file_name().expect("name"));
        assert_eq!(
            repo.index_text(&aliased).as_deref(),
            Some(
                "a
b
c
d
e
"
            )
        );
        let _ = fs::remove_file(&link);
        let _ = fs::remove_dir_all(Path::new(repo.root()));
    }

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
