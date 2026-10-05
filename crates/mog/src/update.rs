//! Finding, downloading and installing new mog releases from GitHub.

use std::{
    env::{self, consts::EXE_SUFFIX},
    fs,
    path::{Component, Path},
    process,
    time::Duration,
};

use minisign_verify::{PublicKey, Signature};
use mog_config::config_dir;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task,
};

/// Where GitHub answers with the newest release.
const LATEST_URL: &str = "https://api.github.com/repos/SpideyZac/mog/releases/latest";

/// Where GitHub answers with a release by tag, with the tag appended.
const TAG_URL: &str = "https://api.github.com/repos/SpideyZac/mog/releases/tags/";

/// The minisign key every release archive must be signed with.
///
/// The secret half lives only in the release workflow, so a release uploaded by anyone else does
/// not install even if its checksum matches.
const RELEASE_KEY: &str = "RWQEyQrj2l2VtRVkLbwHBkVxhMbDdbOGc7wHR8hjR27Ry+epMjmzedE0";

/// The version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The target triple of this build, which names the release archive to download.
const TARGET: &str = env!("MOG_TARGET");

/// The file in the config folder that remembers the last version that ran.
const VERSION_FILE: &str = "last_version";

/// How long talking to the GitHub API may take.
const API_TIMEOUT: Duration = Duration::from_secs(20);

/// How long downloading a release archive may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// A file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    /// The file name, like `mog-v0.2.0-x86_64-pc-windows-msvc.zip`.
    pub name: String,
    /// Where to download it.
    pub browser_download_url: String,
}

/// A release on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    /// The git tag, like `v0.2.0`.
    pub tag_name: String,
    /// The release notes in markdown.
    #[serde(default)]
    pub body: Option<String>,
    /// The release page.
    pub html_url: String,
    /// The attached files.
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// Returns the version without the leading `v`.
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    /// Returns where to download the attached file called `name`.
    fn asset_url(&self, name: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.as_str())
    }
}

/// Something the updater finished.
#[derive(Debug)]
pub enum UpdateEvent {
    /// Looking for a newer release finished.
    Checked {
        /// The newer release, `None` when this is the newest, or why the check failed.
        result: Result<Option<Release>, String>,
        /// Whether someone asked for the check, so being up to date is worth saying.
        manual: bool,
        /// Whether a newer release should be installed right away.
        install: bool,
    },
    /// Fetching the notes of a release finished.
    Notes(Result<Release, String>),
    /// Installing a release finished.
    Installed(Result<Release, String>),
}

/// Talks to GitHub in the background and reports back through [`Updater::event`].
pub struct Updater {
    /// The HTTP client, missing if it could not be built.
    client: Option<Client>,
    /// Where background work sends what it finished.
    sender: UnboundedSender<UpdateEvent>,
    /// What background work finished.
    receiver: UnboundedReceiver<UpdateEvent>,
    /// Whether an install is running.
    installing: bool,
}

impl Updater {
    /// Creates an updater. Nothing is fetched until asked.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let client = Client::builder()
            .user_agent(format!("mog/{VERSION}"))
            .build()
            .ok();
        Self {
            client,
            sender,
            receiver,
            installing: false,
        }
    }

    /// Looks for a release newer than this build.
    pub fn check(&self, manual: bool, install: bool) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = fetch_release(&client, LATEST_URL)
                .await
                .map(|release| is_newer(release.version(), VERSION).then_some(release));
            let _ = sender.send(UpdateEvent::Checked {
                result,
                manual,
                install,
            });
        });
    }

    /// Fetches the notes of the release tagged `tag`, or of the newest release.
    pub fn notes(&self, tag: Option<String>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let url = tag.map_or_else(|| LATEST_URL.to_owned(), |tag| format!("{TAG_URL}{tag}"));
            let _ = sender.send(UpdateEvent::Notes(fetch_release(&client, &url).await));
        });
    }

    /// Downloads `release` and puts it in place of the running mog.
    ///
    /// Returns `false` if an install is already running.
    pub fn install(&mut self, release: Release) -> bool {
        if self.installing {
            return false;
        }
        let Some(client) = self.client.clone() else {
            return false;
        };
        self.installing = true;
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = install(&client, release).await;
            let _ = sender.send(UpdateEvent::Installed(result));
        });
        true
    }

    /// Waits for the next finished piece of background work.
    pub async fn event(&mut self) -> Option<UpdateEvent> {
        let event = self.receiver.recv().await;
        if matches!(event, Some(UpdateEvent::Installed(_))) {
            self.installing = false;
        }
        event
    }
}

/// Updates to the newest release right away, for `mog --update`.
///
/// # Errors
///
/// Returns why the update could not be found or installed.
pub async fn update_now() -> Result<String, String> {
    let client = Client::builder()
        .user_agent(format!("mog/{VERSION}"))
        .build()
        .map_err(|err| err.to_string())?;
    let release = fetch_release(&client, LATEST_URL).await?;
    if !is_newer(release.version(), VERSION) {
        return Ok(format!("mog v{VERSION} is the newest, nothing to do"));
    }
    let release = install(&client, release).await?;
    Ok(format!("updated to mog v{}", release.version()))
}

/// Returns the `major.minor.patch` numbers of `version`, ignoring a `v` and any suffix.
fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

/// Returns whether `candidate` is a newer version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (numbers(candidate), numbers(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Returns whether `path` looks like it was built by cargo in a `target` folder.
fn is_cargo_build(path: &Path) -> bool {
    path.components()
        .any(|part| part == Component::Normal("target".as_ref()))
}

/// Returns why the running mog cannot replace itself, if it cannot.
pub fn cannot_install() -> Option<String> {
    if cfg!(debug_assertions) {
        return Some("this is a debug build, update it with cargo".into());
    }
    if TARGET.is_empty() {
        return Some("this build does not know its platform".into());
    }
    let exe = env::current_exe().ok()?;
    is_cargo_build(&exe)
        .then(|| "this mog was built by cargo, update it with git pull and cargo build".into())
}

/// Returns the archive name of the release `tag` for this platform.
fn archive_name(tag: &str) -> String {
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("mog-{tag}-{TARGET}.{extension}")
}

/// Checks `data` against the first hash in a `sha256sum` style `sums` file.
fn verify(data: &[u8], sums: &str) -> Result<(), String> {
    let expected = sums
        .split_whitespace()
        .next()
        .ok_or("the checksum file is empty")?;
    let actual: String = Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err("the download does not match its checksum".into())
    }
}

/// Checks that `signature` signs `data` with `key` and names the archive `name`.
///
/// Checking the name stops an old signed archive from being passed off as a newer release.
fn verify_signature(data: &[u8], signature: &str, name: &str, key: &str) -> Result<(), String> {
    let key = PublicKey::from_base64(key).map_err(|err| format!("bad release key: {err}"))?;
    let signature =
        Signature::decode(signature).map_err(|err| format!("bad release signature: {err}"))?;
    key.verify(data, &signature, false)
        .map_err(|_| "the download is not signed by the mog release key".to_owned())?;
    let file = format!("file:{name}");
    if signature
        .trusted_comment()
        .split(char::is_whitespace)
        .any(|field| field == file)
    {
        Ok(())
    } else {
        Err("the signature is for a different download".into())
    }
}

/// Returns the `mog` program inside a release zip.
#[cfg(windows)]
fn extract(archive: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::{Cursor, Read};

    use zip::ZipArchive;

    let mut zip = ZipArchive::new(Cursor::new(archive)).map_err(|err| err.to_string())?;
    let wanted = format!("mog{EXE_SUFFIX}");
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|err| err.to_string())?;
        let name = file.name().replace('\\', "/");
        if name.rsplit('/').next() == Some(wanted.as_str()) {
            let mut binary = Vec::new();
            file.read_to_end(&mut binary)
                .map_err(|err| err.to_string())?;
            return Ok(binary);
        }
    }
    Err("the release archive has no mog in it".into())
}

/// Returns the `mog` program inside a release tarball.
#[cfg(not(windows))]
fn extract(archive: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;

    use flate2::read::GzDecoder;
    use tar::Archive;

    let mut tar = Archive::new(GzDecoder::new(archive));
    let wanted = format!("mog{EXE_SUFFIX}");
    for entry in tar.entries().map_err(|err| err.to_string())? {
        let mut entry = entry.map_err(|err| err.to_string())?;
        let is_mog = entry
            .path()
            .ok()
            .and_then(|path| path.file_name().map(|name| name == wanted.as_str()))
            .unwrap_or(false);
        if is_mog {
            let mut binary = Vec::new();
            entry
                .read_to_end(&mut binary)
                .map_err(|err| err.to_string())?;
            return Ok(binary);
        }
    }
    Err("the release archive has no mog in it".into())
}

/// Fetches the release described at the GitHub API `url`.
async fn fetch_release(client: &Client, url: &str) -> Result<Release, String> {
    client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .timeout(API_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| err.to_string())?
        .json::<Release>()
        .await
        .map_err(|err| err.to_string())
}

/// Downloads the file at `url`.
async fn download(client: &Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| err.to_string())?;
    let bytes = response.bytes().await.map_err(|err| err.to_string())?;
    Ok(bytes.to_vec())
}

/// Downloads, checks and installs `release` over the running mog.
async fn install(client: &Client, release: Release) -> Result<Release, String> {
    if let Some(reason) = cannot_install() {
        return Err(reason);
    }
    let name = archive_name(&release.tag_name);
    let url = release
        .asset_url(&name)
        .ok_or_else(|| format!("{} has no build for {TARGET}", release.tag_name))?;
    let sums_url = release
        .asset_url(&format!("{name}.sha256"))
        .ok_or("the release has no checksum, not installing it")?;
    let signature_url = release
        .asset_url(&format!("{name}.minisig"))
        .ok_or("the release is not signed, not installing it")?;
    let archive = download(client, url).await?;
    let sums = download(client, sums_url).await?;
    let signature = download(client, signature_url).await?;
    verify(&archive, &String::from_utf8_lossy(&sums))?;
    verify_signature(
        &archive,
        &String::from_utf8_lossy(&signature),
        &name,
        RELEASE_KEY,
    )?;
    task::spawn_blocking(move || {
        let binary = extract(&archive)?;
        let temp = env::temp_dir().join(format!("mog-update-{}{EXE_SUFFIX}", process::id()));
        fs::write(&temp, binary).map_err(|err| err.to_string())?;
        make_executable(&temp)?;
        let replaced = self_replace::self_replace(&temp).map_err(|err| err.to_string());
        let _ = fs::remove_file(&temp);
        replaced
    })
    .await
    .map_err(|err| err.to_string())??;
    Ok(release)
}

/// Marks the file at `path` as a program anyone can run.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|err| err.to_string())
}

/// Does nothing, since Windows runs any `.exe`.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Records this version as the last one that ran and returns the one before, if any.
pub fn remember_version() -> Option<String> {
    let path = config_dir()?.join(VERSION_FILE);
    let previous = fs::read_to_string(&path)
        .ok()
        .map(|text| text.trim().to_owned());
    if previous.as_deref() != Some(VERSION) {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&path, VERSION);
    }
    previous
}

#[cfg(test)]
/// Tests for the updater.
mod tests {
    use std::path::Path;

    use sha2::{Digest, Sha256};

    use super::{
        RELEASE_KEY, Release, archive_name, extract, is_cargo_build, is_newer, verify,
        verify_signature,
    };

    /// A throwaway public key whose secret half signed [`TEST_SIGNATURE`].
    const TEST_KEY: &str = "RWQev5a1U21teaH2yAvHt4DhJEW4gsX7AhqPJ0jPYD1I/SxsiUwrNrYt";

    /// A signature of `mog` for an archive called `mog-v1.0.0-test.zip`.
    const TEST_SIGNATURE: &str = "untrusted comment: signature from rsign secret key
RUQev5a1U21teYNTMr6HhOVZx2tYd6ia93JtumTA4kOgKIz6x9sEzmOfpHvNHUSSltT3Gjz5kk94qO81qN7D32a0g2UexA8ekAY=
trusted comment: timestamp:1\tfile:mog-v1.0.0-test.zip
38QKpoVAi5dRnkxv1i5xYXLnikSi20fpRBayv3aPD731mMevOFp64FXx6gcQKmKzKOR7L8XGE41T3kiaCQBvCA==
";

    /// Versions compare by number and ignore the `v` and suffixes.
    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("nonsense", "0.1.0"));
    }

    /// Release JSON from GitHub reads into a release.
    #[test]
    fn reads_release_json() {
        let json = r#"{"tag_name":"v0.2.0","name":"v0.2.0","body":"notes","html_url":"https://x",
            "assets":[{"name":"a.zip","browser_download_url":"https://x/a.zip","size":1}],
            "draft":false}"#;
        let release: Release = serde_json::from_str(json).expect("valid release");
        assert_eq!(release.version(), "0.2.0");
        assert_eq!(release.asset_url("a.zip"), Some("https://x/a.zip"));
        assert_eq!(release.asset_url("b.zip"), None);
    }

    /// Archive names follow the release workflow.
    #[test]
    fn names_archives() {
        let name = archive_name("v0.2.0");
        assert!(name.starts_with("mog-v0.2.0-"));
        assert!(name.ends_with(".zip") || name.ends_with(".tar.gz"));
    }

    /// Downloads only pass with the right checksum.
    #[test]
    fn verifies_checksums() {
        let data = b"mog";
        let hash: String = Sha256::digest(data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(verify(data, &format!("{hash}  mog.zip\n")).is_ok());
        assert!(verify(data, &format!("{}  mog.zip", "0".repeat(64))).is_err());
        assert!(verify(data, "").is_err());
    }

    /// Downloads only pass when signed by the key for the archive they claim to be.
    #[test]
    fn verifies_signatures() {
        let name = "mog-v1.0.0-test.zip";
        assert!(verify_signature(b"mog", TEST_SIGNATURE, name, TEST_KEY).is_ok());
        assert!(verify_signature(b"mug", TEST_SIGNATURE, name, TEST_KEY).is_err());
        assert!(verify_signature(b"mog", TEST_SIGNATURE, "mog-v0.1.0-test.zip", TEST_KEY).is_err());
        assert!(verify_signature(b"mog", TEST_SIGNATURE, name, RELEASE_KEY).is_err());
        assert!(verify_signature(b"mog", "nonsense", name, TEST_KEY).is_err());
    }

    /// Builds in a cargo target folder are spotted.
    #[test]
    fn spots_cargo_builds() {
        assert!(is_cargo_build(Path::new("/code/mog/target/release/mog")));
        assert!(!is_cargo_build(Path::new("/usr/local/bin/mog")));
    }

    /// The program comes out of a release archive.
    #[cfg(windows)]
    #[test]
    fn extracts_from_zip() {
        use std::io::{Cursor, Write};

        use zip::{ZipWriter, write::SimpleFileOptions};

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        writer
            .start_file("mog-v1/README.md", options)
            .expect("start");
        writer.write_all(b"readme").expect("write");
        writer.start_file("mog-v1/mog.exe", options).expect("start");
        writer.write_all(b"binary").expect("write");
        let archive = writer.finish().expect("finish").into_inner();
        assert_eq!(extract(&archive).expect("found"), b"binary");
    }

    /// The program comes out of a release archive.
    #[cfg(not(windows))]
    #[test]
    fn extracts_from_tarball() {
        use flate2::{Compression, write::GzEncoder};
        use tar::{Builder, Header};

        let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (path, data) in [
            ("mog-v1/README.md", b"readme".as_slice()),
            ("mog-v1/mog", b"binary"),
        ] {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, data)
                .expect("append");
        }
        let archive = builder.into_inner().expect("tar").finish().expect("gzip");
        assert_eq!(extract(&archive).expect("found"), b"binary");
    }
}
