//! Where a plugin comes from: a folder, a git repository at a version, or a signed archive,
//! and the record of it kept next to the installed plugin so it can be updated.

use std::{
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::{self, Cursor, Read},
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use minisign_verify::{PublicKey, Signature};
use mog_plugin::MANIFEST_FILE;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Archive;
use zip::ZipArchive;

use crate::update::{download, is_newer, version_numbers};

/// The file next to an installed plugin that says where it came from.
pub const RECORD_FILE: &str = ".mog-install.toml";

/// The most a plugin archive may hold once unpacked.
const MAX_UNPACKED: u64 = 128 * 1024 * 1024;

/// The most files a plugin archive may hold.
const MAX_FILES: usize = 10_000;

/// Folders never copied from a plugin folder.
const SKIPPED: &[&str] = &[".git", "__pycache__", "node_modules"];

/// Where a plugin is installed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// A folder on this machine.
    Folder {
        /// The folder.
        path: PathBuf,
    },
    /// A git repository.
    Git {
        /// The repository url.
        url: String,
        /// The tag, branch or commit asked for, the default branch when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    /// A `.tar.gz` or `.zip`, from a url or a file.
    Archive {
        /// The url or file.
        location: String,
    },
}

/// What is known about an installed plugin, kept in [`RECORD_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Where it came from.
    pub source: Source,
    /// The commit that was installed, for git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The minisign public key the archive was checked with, for archives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The SHA-256 of the archive, for archives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// What to trust when installing an archive.
#[derive(Debug, Clone, Default)]
pub struct Trust {
    /// The minisign public key the archive must be signed with.
    pub key: Option<String>,
    /// Install an archive with no signature anyway.
    pub allow_unsigned: bool,
}

/// What an update would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newer {
    /// What is installed, like `v1.2.0` or a short commit.
    pub from: String,
    /// What is available.
    pub to: String,
    /// The version to install to get it, when it changes.
    pub rev: Option<String>,
}

/// Returns whether `source` names a plugin archive.
pub fn is_archive(source: &str) -> bool {
    let lower = source.to_lowercase();
    [".tar.gz", ".tgz", ".zip"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Returns whether `source` looks like a git url rather than a folder.
pub fn is_git_url(source: &str) -> bool {
    source.starts_with("https://")
        || source.starts_with("http://")
        || source.starts_with("git@")
        || source.starts_with("ssh://")
        || source.split('#').next().is_some_and(|url| url.ends_with(".git"))
}

/// Returns whether `rev` looks like a commit hash rather than a tag or branch.
fn is_commit(rev: &str) -> bool {
    (7..=40).contains(&rev.len()) && rev.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// Returns the first 8 chars of a commit.
fn short(commit: &str) -> &str {
    commit.get(..8).unwrap_or(commit)
}

impl Source {
    /// Reads what `mog plugin install` was given: a folder, a git url with an optional
    /// `#version`, or an archive. `rev` is a version from `--rev`.
    ///
    /// # Errors
    ///
    /// Returns an error if a version is given for something that is not git.
    pub fn parse(source: &str, rev: Option<&str>) -> Result<Self> {
        let parsed = if is_archive(source) {
            Self::Archive {
                location: source.to_owned(),
            }
        } else if is_git_url(source) {
            let (url, pinned) = match source.rsplit_once('#') {
                Some((url, pinned)) if !pinned.is_empty() => (url, Some(pinned)),
                _ => (source, None),
            };
            if let (Some(pinned), Some(rev)) = (pinned, rev)
                && pinned != rev
            {
                bail!("{source} names version {pinned} but --rev says {rev}");
            }
            Self::Git {
                url: url.to_owned(),
                rev: rev.or(pinned).map(str::to_owned),
            }
        } else {
            Self::Folder {
                path: PathBuf::from(source),
            }
        };
        if rev.is_some() && !matches!(parsed, Self::Git { .. }) {
            bail!("--rev only works for git urls");
        }
        Ok(parsed)
    }

    /// Returns the source with a different git version, or itself for others.
    pub fn at(&self, new_rev: Option<String>) -> Self {
        match (self, new_rev) {
            (Self::Git { url, .. }, Some(rev)) => Self::Git {
                url: url.clone(),
                rev: Some(rev),
            },
            (other, _) => other.clone(),
        }
    }
}

impl Display for Source {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Folder { path } => write!(f, "{}", path.display()),
            Self::Git { url, rev: None } => write!(f, "{url}"),
            Self::Git {
                url,
                rev: Some(rev),
            } => write!(f, "{url}#{rev}"),
            Self::Archive { location } => write!(f, "{location}"),
        }
    }
}

impl Record {
    /// Reads the record of the plugin in `folder`, if it has one.
    pub fn read(folder: &Path) -> Option<Self> {
        let text = fs::read_to_string(folder.join(RECORD_FILE)).ok()?;
        toml::from_str(&text).ok()
    }

    /// Writes the record into `folder`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn write(&self, folder: &Path) -> Result<()> {
        let text = format!(
            "# where mog installed this plugin from, used by mog plugin update\n{}",
            toml::to_string(self)?
        );
        fs::write(folder.join(RECORD_FILE), text)
            .with_context(|| format!("could not write {}", folder.join(RECORD_FILE).display()))
    }
}

/// Copies the folder `from` into `to`, skipping [`SKIPPED`] folders.
///
/// # Errors
///
/// Returns an error if a file cannot be copied.
pub fn copy_folder(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            let name = entry.file_name();
            if !SKIPPED.iter().any(|skipped| name == *skipped) {
                copy_folder(&entry.path(), &target)?;
            }
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Runs git with `args`, returning what it printed.
fn git(args: &[&str], dir: Option<&Path>) -> Result<String> {
    let mut command = Command::new("git");
    command.args(["-c", "advice.detachedHead=false"]).args(args);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .context("could not run git, is it installed?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args[0], stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Clones `url` at `rev` into `staging`, leaving no `.git` folder, and returns the commit.
fn clone(url: &str, rev: Option<&str>, staging: &Path) -> Result<String> {
    let into = staging.to_string_lossy();
    let shallow = match rev {
        Some(rev) if is_commit(rev) => None,
        Some(rev) => Some(git(
            &[
                "clone", "--quiet", "--depth", "1", "--branch", rev, url, &into,
            ],
            None,
        )),
        None => Some(git(&["clone", "--quiet", "--depth", "1", url, &into], None)),
    };
    // a commit cannot be cloned shallow by name, so fetch everything and check it out
    if !shallow.as_ref().is_some_and(Result::is_ok) {
        let _ = fs::remove_dir_all(staging);
        git(&["clone", "--quiet", url, &into], None)?;
        if let Some(rev) = rev {
            git(&["checkout", "--quiet", rev], Some(staging))
                .with_context(|| format!("{url} has no version {rev}"))?;
        }
    }
    let commit = git(&["rev-parse", "HEAD"], Some(staging))?;
    fs::remove_dir_all(staging.join(".git")).context("could not remove the .git folder")?;
    Ok(commit)
}

/// Reads `location`, downloading it if it is a url. Returns `None` if it does not exist.
async fn read_location(client: &Client, location: &str) -> Result<Option<Vec<u8>>> {
    if location.starts_with("https://") || location.starts_with("http://") {
        return match download(client, location).await {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.contains("404") => Ok(None),
            Err(err) => Err(anyhow!("could not download {location}: {err}")),
        };
    }
    match fs::read(location) {
        Ok(data) => Ok(Some(data)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow!("could not read {location}: {err}")),
    }
}

/// Checks that `signature` signs `data` with `key`, and that a file name in its trusted
/// comment is `name`, so an old signed archive cannot be passed off under a newer name.
///
/// # Errors
///
/// Returns why the signature does not hold.
pub fn check_signature(data: &[u8], signature: &str, key: &str, name: &str) -> Result<()> {
    let key = PublicKey::from_base64(key.trim()).map_err(|err| anyhow!("bad key: {err}"))?;
    let signature = Signature::decode(signature).map_err(|err| anyhow!("bad signature: {err}"))?;
    key.verify(data, &signature, false)
        .map_err(|_| anyhow!("the archive is not signed with that key"))?;
    let named = signature
        .trusted_comment()
        .split(char::is_whitespace)
        .filter_map(|field| field.strip_prefix("file:"))
        .collect::<Vec<_>>();
    if !named.is_empty() && !named.contains(&name) {
        bail!("the signature is for {}, not {name}", named.join(", "));
    }
    Ok(())
}

/// Returns `path` inside an archive as a safe relative path, or `None` if it would land outside
/// the folder it is unpacked into.
fn safe_path(path: &Path) -> Option<PathBuf> {
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!safe.as_os_str().is_empty()).then_some(safe)
}

/// Keeps count of what an archive unpacks, failing once it is too much.
#[derive(Debug, Default)]
struct Budget {
    /// Bytes written so far.
    bytes: u64,
    /// Files written so far.
    files: usize,
}

impl Budget {
    /// Writes the file `reader` to `target`, counting it.
    fn write(&mut self, reader: impl Read, target: &Path) -> Result<()> {
        self.files += 1;
        if self.files > MAX_FILES {
            bail!("the archive has more than {MAX_FILES} files");
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let left = MAX_UNPACKED - self.bytes;
        let written = io::copy(&mut reader.take(left + 1), &mut File::create(target)?)?;
        self.bytes += written;
        if self.bytes > MAX_UNPACKED {
            bail!("the archive unpacks to more than {} MB", MAX_UNPACKED >> 20);
        }
        Ok(())
    }
}

/// Unpacks the archive `data` called `name` into `staging`.
fn unpack(data: &[u8], name: &str, staging: &Path) -> Result<()> {
    let mut budget = Budget::default();
    if name.to_lowercase().ends_with(".zip") {
        let mut zip = ZipArchive::new(Cursor::new(data)).context("not a zip archive")?;
        for index in 0..zip.len() {
            let file = zip.by_index(index)?;
            if file.is_dir() {
                continue;
            }
            let path = file
                .enclosed_name()
                .and_then(|path| safe_path(&path))
                .ok_or_else(|| anyhow!("the archive has an unsafe path {}", file.name()))?;
            budget.write(file, &staging.join(path))?;
        }
    } else {
        let mut tar = Archive::new(GzDecoder::new(data));
        for entry in tar.entries().context("not a tar.gz archive")? {
            let entry = entry?;
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                continue;
            }
            if !kind.is_file() {
                bail!("the archive has a link or special file, which plugins do not need");
            }
            let path = entry.path()?.into_owned();
            let safe = safe_path(&path)
                .ok_or_else(|| anyhow!("the archive has an unsafe path {}", path.display()))?;
            budget.write(entry, &staging.join(safe))?;
        }
    }
    // archives usually hold one folder named after the plugin, so use it as the plugin
    if !staging.join(MANIFEST_FILE).is_file() {
        let entries: Vec<PathBuf> = fs::read_dir(staging)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        if let [only] = entries.as_slice()
            && only.join(MANIFEST_FILE).is_file()
        {
            let inner = staging.with_extension("inner");
            fs::rename(only, &inner)?;
            fs::remove_dir_all(staging)?;
            fs::rename(&inner, staging)?;
        }
    }
    Ok(())
}

/// Returns the hex SHA-256 of `data`.
fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fetches an archive, checks its signature and unpacks it into `staging`.
async fn fetch_archive(location: &str, staging: &Path, trust: &Trust) -> Result<Record> {
    let client = Client::builder()
        .user_agent(format!("mog/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let data = read_location(&client, location)
        .await?
        .ok_or_else(|| anyhow!("{location} does not exist"))?;
    let signature = read_location(&client, &format!("{location}.minisig")).await?;
    let name = location
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(location)
        .to_owned();
    match (&trust.key, signature) {
        (Some(key), Some(signature)) => {
            let signature = String::from_utf8(signature).context("the signature is not text")?;
            check_signature(&data, &signature, key, &name)?;
        }
        (Some(_), None) => bail!("{location}.minisig is missing, so the archive cannot be checked"),
        (None, _) if trust.allow_unsigned => {}
        (None, _) => bail!(
            "archives must be signed: pass --key with the author's minisign public key, or \
             --allow-unsigned to install it without checking"
        ),
    }
    fs::create_dir_all(staging)?;
    unpack(&data, &name, staging)?;
    Ok(Record {
        source: Source::Archive {
            location: location.to_owned(),
        },
        commit: None,
        key: trust.key.clone(),
        sha256: Some(sha256(&data)),
    })
}

/// Puts the plugin from `source` into the new folder `staging` and returns its record.
///
/// # Errors
///
/// Returns why it could not be fetched, checked or unpacked.
pub async fn fetch(source: &Source, staging: &Path, trust: &Trust) -> Result<Record> {
    match source {
        Source::Folder { path } => {
            if !path.join(MANIFEST_FILE).is_file() {
                bail!("{} has no {MANIFEST_FILE}", path.display());
            }
            copy_folder(path, staging)
                .with_context(|| format!("could not copy {}", path.display()))?;
            let path = path.canonicalize().unwrap_or_else(|_| path.clone());
            Ok(Record {
                source: Source::Folder { path },
                commit: None,
                key: None,
                sha256: None,
            })
        }
        Source::Git { url, rev } => {
            let commit = clone(url, rev.as_deref(), staging)?;
            Ok(Record {
                source: source.clone(),
                commit: Some(commit),
                key: None,
                sha256: None,
            })
        }
        Source::Archive { location } => fetch_archive(location, staging, trust).await,
    }
}

/// Returns the refs of the repository at `url` as `(name, commit)`, like `refs/tags/v1.0.0`.
fn remote_refs(url: &str) -> Result<Vec<(String, String)>> {
    let listed = git(&["ls-remote", url], None)?;
    Ok(listed
        .lines()
        .filter_map(|line| {
            let (commit, name) = line.split_once(char::is_whitespace)?;
            Some((name.trim().to_owned(), commit.to_owned()))
        })
        .collect())
}

/// Returns what an update of the plugin installed as `record` would bring, or `None` when it is
/// current, pinned to a commit, or from a folder.
///
/// # Errors
///
/// Returns why the repository could not be asked.
pub fn newer(record: &Record) -> Result<Option<Newer>> {
    let Source::Git { url, rev } = &record.source else {
        return Ok(None);
    };
    let installed = record.commit.as_deref().unwrap_or_default();
    match rev.as_deref() {
        Some(rev) if is_commit(rev) => Ok(None),
        Some(rev) if version_numbers(rev).is_some() => {
            let newest = remote_refs(url)?
                .into_iter()
                .filter_map(|(name, _)| {
                    let tag = name.strip_prefix("refs/tags/")?;
                    (!tag.ends_with("^{}")).then(|| tag.to_owned())
                })
                .filter(|tag| version_numbers(tag).is_some())
                .fold(rev.to_owned(), |best, tag| {
                    if is_newer(&tag, &best) { tag } else { best }
                });
            Ok((newest != rev).then(|| Newer {
                from: rev.to_owned(),
                to: newest.clone(),
                rev: Some(newest),
            }))
        }
        branch => {
            let wanted = branch.map_or_else(
                || "HEAD".to_owned(),
                |branch| format!("refs/heads/{branch}"),
            );
            let head = remote_refs(url)?
                .into_iter()
                .find(|(name, _)| *name == wanted)
                .map(|(_, commit)| commit)
                .ok_or_else(|| anyhow!("{url} has no {wanted}"))?;
            Ok((head != installed).then(|| Newer {
                from: short(installed).to_owned(),
                to: short(&head).to_owned(),
                rev: None,
            }))
        }
    }
}

#[cfg(test)]
/// Tests for plugin sources.
mod tests {
    use std::{
        env, fs,
        io::{Cursor, Write as _},
        path::Path,
        process,
    };

    use flate2::{Compression, write::GzEncoder};
    use tar::{Builder, Header};
    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::{
        Record, Source, Trust, check_signature, fetch, is_archive, is_git_url, safe_path, unpack,
    };

    /// A throwaway public key whose secret half signed [`TEST_SIGNATURE`], from the updater.
    const TEST_KEY: &str = "RWQev5a1U21teaH2yAvHt4DhJEW4gsX7AhqPJ0jPYD1I/SxsiUwrNrYt";

    /// A signature of `mog` for a file called `mog-v1.0.0-test.zip`.
    const TEST_SIGNATURE: &str = "untrusted comment: signature from rsign secret key
RUQev5a1U21teYNTMr6HhOVZx2tYd6ia93JtumTA4kOgKIz6x9sEzmOfpHvNHUSSltT3Gjz5kk94qO81qN7D32a0g2UexA8ekAY=
trusted comment: timestamp:1\tfile:mog-v1.0.0-test.zip
38QKpoVAi5dRnkxv1i5xYXLnikSi20fpRBayv3aPD731mMevOFp64FXx6gcQKmKzKOR7L8XGE41T3kiaCQBvCA==
";

    /// Signatures must be by the key, of the data, for the file they claim to be.
    #[test]
    fn checks_signatures() {
        let name = "mog-v1.0.0-test.zip";
        assert!(check_signature(b"mog", TEST_SIGNATURE, TEST_KEY, name).is_ok());
        assert!(check_signature(b"mug", TEST_SIGNATURE, TEST_KEY, name).is_err());
        assert!(check_signature(b"mog", TEST_SIGNATURE, TEST_KEY, "mog-v0.9.0-test.zip").is_err());
        let other = "RWQEyQrj2l2VtRVkLbwHBkVxhMbDdbOGc7wHR8hjR27Ry+epMjmzedE0";
        assert!(check_signature(b"mog", TEST_SIGNATURE, other, name).is_err());
        assert!(check_signature(b"mog", "nonsense", TEST_KEY, name).is_err());
    }

    /// An archive without a key is refused unless unsigned ones are allowed, and a key needs a
    /// signature next to the archive.
    #[tokio::test]
    async fn needs_signed_archives() {
        let base = env::temp_dir().join(format!("mog-signed-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("dir");
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("plugin.toml", SimpleFileOptions::default())
            .expect("file");
        zip.write_all(b"name = \"hi\"").expect("write");
        let archive = base.join("hi-1.0.0.zip");
        fs::write(&archive, zip.finish().expect("zip").into_inner()).expect("archive");
        let source = Source::Archive {
            location: archive.to_string_lossy().into_owned(),
        };
        let refused = fetch(&source, &base.join("a"), &Trust::default()).await;
        assert!(refused.is_err_and(|err| err.to_string().contains("--key")));
        let keyed = Trust {
            key: Some(TEST_KEY.into()),
            allow_unsigned: false,
        };
        let missing = fetch(&source, &base.join("b"), &keyed).await;
        assert!(missing.is_err_and(|err| err.to_string().contains("minisig")));
        let allowed = Trust {
            key: None,
            allow_unsigned: true,
        };
        let record = fetch(&source, &base.join("c"), &allowed)
            .await
            .expect("unpacked");
        assert!(record.key.is_none() && record.sha256.is_some());
        assert!(base.join("c/plugin.toml").is_file());
        // a signature for different data is refused
        fs::write(base.join("hi-1.0.0.zip.minisig"), TEST_SIGNATURE).expect("signature");
        let forged = fetch(&source, &base.join("d"), &keyed).await;
        assert!(forged.is_err_and(|err| err.to_string().contains("not signed")));
        let _ = fs::remove_dir_all(base);
    }

    /// Git urls carry a version after `#`, archives and folders are told apart.
    #[test]
    fn reads_sources() {
        assert_eq!(
            Source::parse("https://github.com/me/plugin#v1.2.0", None).expect("git"),
            Source::Git {
                url: "https://github.com/me/plugin".into(),
                rev: Some("v1.2.0".into())
            }
        );
        assert_eq!(
            Source::parse("git@github.com:me/plugin.git", Some("main")).expect("git"),
            Source::Git {
                url: "git@github.com:me/plugin.git".into(),
                rev: Some("main".into())
            }
        );
        assert!(Source::parse("https://x/p#v1", Some("v2")).is_err());
        assert!(matches!(
            Source::parse("https://example.com/todo-1.0.0.tar.gz", None),
            Ok(Source::Archive { .. })
        ));
        assert!(Source::parse("./plugins/words", Some("v1")).is_err());
        assert!(is_archive("x.ZIP") && !is_archive("x.git"));
        assert!(is_git_url("git@github.com:me/plugin.git") && !is_git_url("./words"));
    }

    /// The record survives a round trip through its file.
    #[test]
    fn keeps_records() {
        let dir = env::temp_dir().join(format!("mog-record-{}", process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let record = Record {
            source: Source::Git {
                url: "https://x/p".into(),
                rev: Some("v1.0.0".into()),
            },
            commit: Some("abc".into()),
            key: None,
            sha256: None,
        };
        record.write(&dir).expect("written");
        assert_eq!(Record::read(&dir), Some(record));
        let _ = fs::remove_dir_all(dir);
    }

    /// Paths that climb out of the folder or are absolute are refused.
    #[test]
    fn refuses_unsafe_paths() {
        assert_eq!(
            safe_path(Path::new("a/./b.py")).as_deref(),
            Some(Path::new("a/b.py"))
        );
        assert!(safe_path(Path::new("../evil")).is_none());
        assert!(safe_path(Path::new("/etc/passwd")).is_none());
    }

    /// Both archive kinds unpack, and a single top folder becomes the plugin folder.
    #[test]
    fn unpacks_archives() {
        let base = env::temp_dir().join(format!("mog-unpack-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("todo/plugin.toml", SimpleFileOptions::default())
            .expect("file");
        zip.write_all(b"name = \"todo\"").expect("write");
        let zipped = zip.finish().expect("zip").into_inner();
        let from_zip = base.join("zip");
        unpack(&zipped, "todo.zip", &from_zip).expect("unpacked");
        assert!(from_zip.join("plugin.toml").is_file());
        let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut header = Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "plugin.toml", &b"x = 1"[..])
            .expect("append");
        let tarred = tar.into_inner().expect("tar").finish().expect("gz");
        let from_tar = base.join("tar");
        unpack(&tarred, "todo.tar.gz", &from_tar).expect("unpacked");
        assert!(from_tar.join("plugin.toml").is_file());
        let _ = fs::remove_dir_all(base);
    }
}
