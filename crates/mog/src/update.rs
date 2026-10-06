//! Finding, downloading and installing new mog releases from GitHub.

use std::{
    env::{self, consts::EXE_SUFFIX},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
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

/// The minisign keys a release archive may be signed with.
///
/// The secret halves live only in the release workflow, so a release uploaded by anyone else does
/// not install even if its checksum matches. To rotate, ship a release that trusts the old and the
/// new key, sign later releases with the new one, then drop the old one.
const RELEASE_KEYS: &[&str] = &["RWQxm0+WtCFyKauL+lbkHe5SYv81b5BxDzeYRCmfEwgPf062L0qEbDxb",];

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

/// The most a download or the program inside it may weigh, in bytes.
const MAX_SIZE: u64 = 256 * 1024 * 1024;

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
pub fn version_numbers(version: &str) -> Option<(u64, u64, u64)> {
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
    match (version_numbers(candidate), version_numbers(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Returns whether `path` looks like it was built by cargo in a `target` folder.
fn is_cargo_build(path: &Path) -> bool {
    path.components()
        .any(|part| part == Component::Normal("target".as_ref()))
}

/// Returns the package manager that put `path` there, if one did.
fn package_manager(path: &Path) -> Option<&'static str> {
    let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if text.contains("/cellar/") || text.contains("/homebrew/") || text.contains("/linuxbrew/") {
        Some("homebrew, update it with brew upgrade mog")
    } else if text.contains("/winget/packages/") || text.contains("/winget/links/") {
        Some("winget, update it with winget upgrade mog")
    } else if text.starts_with("/usr/bin/") {
        Some("your package manager, update it there")
    } else if text.contains("/scoop/") {
        Some("scoop, update it with scoop update mog")
    } else {
        None
    }
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
    // a package manager would fight over a binary that changes under it
    if let Some(manager) = package_manager(&exe) {
        return Some(format!("this mog came from {manager}"));
    }
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
fn verify_signature(data: &[u8], signature: &str, name: &str, keys: &[&str]) -> Result<(), String> {
    let signature =
        Signature::decode(signature).map_err(|err| format!("bad release signature: {err}"))?;
    let mut signed = false;
    for key in keys {
        let key = PublicKey::from_base64(key).map_err(|err| format!("bad release key: {err}"))?;
        if key.verify(data, &signature, false).is_ok() {
            signed = true;
            break;
        }
    }
    if !signed {
        return Err("the download is not signed by a mog release key".to_owned());
    }
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

/// Reads all of `reader`, failing once it passes `limit` bytes so a bomb cannot fill memory.
fn read_capped(reader: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut binary = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut binary)
        .map_err(|err| err.to_string())?;
    if binary.len() as u64 > limit {
        return Err("the program in the release archive is too big".into());
    }
    Ok(binary)
}

/// Returns the `mog` program inside a release zip.
#[cfg(windows)]
fn extract(archive: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Cursor;

    use zip::ZipArchive;

    let mut zip = ZipArchive::new(Cursor::new(archive)).map_err(|err| err.to_string())?;
    let wanted = format!("mog{EXE_SUFFIX}");
    for index in 0..zip.len() {
        let file = zip.by_index(index).map_err(|err| err.to_string())?;
        let name = file.name().replace('\\', "/");
        if name.rsplit('/').next() == Some(wanted.as_str()) {
            return read_capped(file, MAX_SIZE);
        }
    }
    Err("the release archive has no mog in it".into())
}

/// Returns the `mog` program inside a release tarball.
#[cfg(not(windows))]
fn extract(archive: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let mut tar = Archive::new(GzDecoder::new(archive));
    let wanted = format!("mog{EXE_SUFFIX}");
    for entry in tar.entries().map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let is_mog = entry
            .path()
            .ok()
            .and_then(|path| path.file_name().map(|name| name == wanted.as_str()))
            .unwrap_or(false);
        if is_mog {
            return read_capped(entry, MAX_SIZE);
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

/// Downloads the file at `url`, up to 256 MB.
///
/// # Errors
///
/// Returns why it could not be downloaded, with the HTTP status if there was one.
pub async fn download(client: &Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| err.to_string())?;
    let too_big = || format!("{url} is bigger than {} MB", MAX_SIZE / 1024 / 1024);
    if response.content_length().is_some_and(|len| len > MAX_SIZE) {
        return Err(too_big());
    }
    let mut data = Vec::new();
    // the length header can lie, so count what actually arrives
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        data.extend_from_slice(&chunk);
        if data.len() as u64 > MAX_SIZE {
            return Err(too_big());
        }
    }
    Ok(data)
}

/// Writes `binary` to a new file in `dir` that nobody else could have made first.
///
/// The file sits next to the running program so the swap is a rename on the same disk, and
/// `create_new` refuses a file or link someone planted under the same name.
fn stage(binary: &[u8], dir: &Path) -> Result<PathBuf, String> {
    for attempt in 0..100 {
        let path = dir.join(format!(
            ".mog-update-{}-{attempt}{EXE_SUFFIX}",
            process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        owner_only(&mut options);
        let mut file: File = match options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("could not write {}: {err}", path.display())),
        };
        let written = file
            .write_all(binary)
            .and_then(|()| file.sync_all())
            .map_err(|err| err.to_string());
        if let Err(err) = written.and_then(|()| make_executable(&path)) {
            let _ = fs::remove_file(&path);
            return Err(err);
        }
        return Ok(path);
    }
    Err("could not find a free name for the update".into())
}

/// Makes files opened with `options` readable only by their owner.
#[cfg(unix)]
fn owner_only(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o700);
}

/// Does nothing, since files in the user's own folders are already theirs on Windows.
#[cfg(not(unix))]
fn owner_only(_options: &mut OpenOptions) {}

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
        RELEASE_KEYS,
    )?;
    task::spawn_blocking(move || {
        let binary = extract(&archive)?;
        let exe = env::current_exe().map_err(|err| err.to_string())?;
        let dir = exe.parent().ok_or("mog is not in a folder")?;
        let temp = stage(&binary, dir)?;
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
    use std::{env, fs, path::Path, process};

    use reqwest::Client;
    use sha2::{Digest, Sha256};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::{
        MAX_SIZE, RELEASE_KEYS, Release, archive_name, download, extract, is_cargo_build, is_newer,
        package_manager, read_capped, stage, verify, verify_signature,
    };

    /// Serves one HTTP response with `head` as the headers and `body` after them, returning the
    /// url to fetch.
    async fn serve_once(head: String, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}/mog.zip", listener.local_addr().expect("addr"));
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        });
        url
    }

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
        assert!(verify_signature(b"mog", TEST_SIGNATURE, name, &[TEST_KEY]).is_ok());
        assert!(verify_signature(b"mug", TEST_SIGNATURE, name, &[TEST_KEY]).is_err());
        assert!(
            verify_signature(b"mog", TEST_SIGNATURE, "mog-v0.1.0-test.zip", &[TEST_KEY]).is_err()
        );
        assert!(verify_signature(b"mog", TEST_SIGNATURE, name, RELEASE_KEYS).is_err());
        // a release that trusts an old and a new key takes either
        let rotating = [RELEASE_KEYS[0], TEST_KEY];
        assert!(verify_signature(b"mog", TEST_SIGNATURE, name, &rotating).is_ok());
        assert!(verify_signature(b"mog", "nonsense", name, &[TEST_KEY]).is_err());
    }

    /// Installs from package managers are left to them.
    #[test]
    fn spots_package_managers() {
        assert!(package_manager(Path::new("/opt/homebrew/Cellar/mog/0.3.0/bin/mog")).is_some());
        assert!(package_manager(Path::new("/usr/bin/mog")).is_some());
        assert!(
            package_manager(Path::new(
                r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\SpideyZac.mog\mog.exe"
            ))
            .is_some()
        );
        assert!(package_manager(Path::new("/home/me/.local/bin/mog")).is_none());
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

    /// The update is staged in a new file and never through one already there.
    #[test]
    fn stages_into_a_new_file() {
        let dir = env::temp_dir().join(format!("mog-stage-test-{}", process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let planted = dir.join(format!(
            ".mog-update-{}-0{}",
            process::id(),
            env::consts::EXE_SUFFIX
        ));
        fs::write(&planted, b"planted").expect("plant");
        let staged = stage(b"binary", &dir).expect("staged");
        assert_ne!(staged, planted);
        assert_eq!(fs::read(&staged).expect("read"), b"binary");
        assert_eq!(fs::read(&planted).expect("read"), b"planted");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Reading out of an archive stops at the size cap.
    #[test]
    fn caps_extracted_size() {
        let data = vec![0; 16];
        assert_eq!(read_capped(data.as_slice(), 16).expect("fits"), data);
        assert!(read_capped(data.as_slice(), 15).is_err());
    }

    /// A download comes through whole, and one that says it is too big is refused.
    #[tokio::test]
    async fn downloads_with_a_cap() {
        let client = Client::new();
        let body = b"archive".to_vec();
        let ok = |len: u64| {
            format!("HTTP/1.1 200 OK\r\ncontent-length: {len}\r\nconnection: close\r\n\r\n")
        };
        let url = serve_once(ok(7), body.clone()).await;
        assert_eq!(download(&client, &url).await.expect("downloaded"), body);
        let url = serve_once(ok(MAX_SIZE + 1), Vec::new()).await;
        assert!(download(&client, &url).await.is_err());
        let missing = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n";
        let url = serve_once(missing.into(), Vec::new()).await;
        assert!(download(&client, &url).await.is_err());
    }
}
