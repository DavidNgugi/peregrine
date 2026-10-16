//! The managed Prowlarr component pack (docs/adr/0024).
//!
//! Prowlarr is GPL-3.0-or-later, so it never links into this process (docs/adr/0009): it is
//! downloaded on the user's explicit action, verified against a size and SHA-256 compiled into
//! this binary, extracted, and run as a child process bound to loopback. The app then reads
//! Prowlarr's own config.xml for the port and the API key it generated, and stores the key in the
//! keyring under the same names the indexer settings already use, so a managed instance and a
//! user-supplied one are indistinguishable to the search path.
//!
//! The pinned values are in this file, never fetched at runtime (docs/adr/0022 section 1). The
//! manifest entry that carries the licence and the written offer of source is sidecars.toml.
//!
//! Nothing here runs at start-up: install and Supervisor::start are explicit calls.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use peregrine_secrets::{Secret, SecretStore, SecretStoreError};
use sha2::{Digest, Sha256};

use crate::indexers::{api_key_key, endpoint_key};

/// The pinned Prowlarr release this build installs. One version per release (docs/adr/0024).
pub const VERSION: &str = "2.6.5.5623";

/// How long to wait for Prowlarr to answer its own health check after it is spawned.
const READY_TIMEOUT: Duration = Duration::from_secs(90);
/// How long to keep re-reading config.xml after /ping answers. Prowlarr can answer the health
/// check before it has written its key, so a single read is a race a real run lost.
const CONFIG_TIMEOUT: Duration = Duration::from_secs(15);
/// How often to ask while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Prowlarr's port before it has written a config.xml of its own.
const DEFAULT_PORT: u16 = 9696;
/// What the release request identifies itself as (AGENTS R24).
const USER_AGENT: &str = concat!("Peregrine/", env!("CARGO_PKG_VERSION"));

/// How one platform's pinned archive is packed.
///
/// Only one variant is ever constructed on a given target, because the pinned artefact names one
/// archive per OS; the other is the other OS's archive and is dead on this build.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Archive {
    /// A .zip, as macOS and Windows publish.
    Zip,
    /// A .tar.gz, as Linux publishes.
    TarGz,
}

/// One pinned release artefact: where it comes from and what it must hash to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Artifact {
    url: &'static str,
    size: u64,
    sha256: &'static str,
    archive: Archive,
}

/// The artefact pinned for this OS/arch, or a typed refusal. Every value is compiled in.
#[allow(unreachable_code, clippy::needless_return)]
fn platform() -> Result<Artifact, ProwlarrError> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        const URL: &str = "https://github.com/Prowlarr/Prowlarr/releases/download/v2.6.5.5623/Prowlarr.master.2.6.5.5623.osx-app-core-arm64.zip";
        const SIZE: u64 = 58_827_010;
        const SHA256: &str = "cd3401b34a11256fcb5e49c26e20382ca4d3f14c1d2c102247a4a46e66a5ab24";
        return Ok(Artifact {
            url: URL,
            size: SIZE,
            sha256: SHA256,
            archive: Archive::Zip,
        });
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        const URL: &str = "https://github.com/Prowlarr/Prowlarr/releases/download/v2.6.5.5623/Prowlarr.master.2.6.5.5623.linux-core-x64.tar.gz";
        const SIZE: u64 = 96_747_834;
        const SHA256: &str = "c0824e9f0e9c79e085882f6143041be5c2cde0519baa59e9477d9e4dcbe1b5e3";
        return Ok(Artifact {
            url: URL,
            size: SIZE,
            sha256: SHA256,
            archive: Archive::TarGz,
        });
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        const URL: &str = "https://github.com/Prowlarr/Prowlarr/releases/download/v2.6.5.5623/Prowlarr.master.2.6.5.5623.windows-core-x64.zip";
        const SIZE: u64 = 135_771_253;
        const SHA256: &str = "7d8dfc745ab784fa3d9cd56522b29f2a6b53a2b72701adc295fe148f9d466699";
        return Ok(Artifact {
            url: URL,
            size: SIZE,
            sha256: SHA256,
            archive: Archive::Zip,
        });
    }
    Err(ProwlarrError::UnsupportedPlatform)
}

/// Whether this build has a pinned Prowlarr for the running platform.
///
/// A settings screen asks before it offers an Install button, so an unsupported OS is a fact the
/// user is told rather than a button that fails.
#[must_use]
pub fn is_supported() -> bool {
    platform().is_ok()
}

/// The pinned size for this platform, or `None` where no pack exists. The install job reports
/// progress against it, so the total is the same number the download verifies against.
#[must_use]
pub fn pinned_size() -> Option<u64> {
    platform().ok().map(|artifact| artifact.size)
}

/// Why the managed Prowlarr could not be installed or started.
///
/// No variant carries a key, a path into a secret, or a URL: the release URL is public, but the
/// rule the indexer module keeps is that a secret never reaches a log or a message, and carrying
/// none of the request's details here is the simplest way to keep it.
#[derive(Debug, thiserror::Error)]
pub enum ProwlarrError {
    /// No pinned Prowlarr build exists for this OS/arch.
    #[error("this platform has no pinned Prowlarr build")]
    UnsupportedPlatform,
    /// The release could not be reached or did not answer as an archive.
    #[error("the Prowlarr release could not be downloaded")]
    Transport,
    /// The streamed byte count did not equal the pinned size.
    #[error("the Prowlarr download did not match its pinned size")]
    SizeMismatch,
    /// The streamed bytes did not hash to the pinned SHA-256.
    #[error("the Prowlarr download did not match its pinned checksum")]
    ChecksumMismatch,
    /// The archive could not be read or unpacked.
    #[error("the Prowlarr archive could not be unpacked")]
    Archive,
    /// A file could not be written while staging.
    #[error("the Prowlarr pack could not be written to disk")]
    Io(#[source] std::io::Error),
    /// The archive had no executable at the path this platform expects.
    #[error("the Prowlarr archive did not contain its executable")]
    MissingExecutable,
    /// The macOS bundle could not be un-quarantined or re-signed.
    #[error("the Prowlarr macOS bundle could not be prepared")]
    Bundle,
    /// A managed instance is already answering on the loopback port.
    #[error("a managed Prowlarr is already running")]
    AlreadyRunning,
    /// Prowlarr did not answer GET /ping inside the timeout.
    #[error("Prowlarr did not answer its health check in time")]
    NotReady,
    /// Prowlarr's config.xml had no readable API key.
    #[error("Prowlarr's config.xml did not hold a key")]
    Config,
    /// The keyring refused the key.
    #[error("the keyring refused the Prowlarr key")]
    Secret(#[source] SecretStoreError),
    /// The blocking keyring write did not finish.
    #[error("the keyring write did not finish")]
    Task,
}

impl From<SecretStoreError> for ProwlarrError {
    fn from(source: SecretStoreError) -> Self {
        Self::Secret(source)
    }
}

/// A verified, unpacked Prowlarr under the data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    root: PathBuf,
}

impl Installed {
    /// The directory the pinned version was unpacked into.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The executable this platform runs.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        executable_at(&self.root)
    }

    /// Whether the executable is present, which is also how a complete install is recognised.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.executable().is_file()
    }
}

/// Where this platform's executable sits inside an unpacked pack.
#[allow(unreachable_code, clippy::needless_return)]
fn executable_at(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return root
            .join("Prowlarr.app")
            .join("Contents")
            .join("MacOS")
            .join("Prowlarr");
    }
    #[cfg(target_os = "linux")]
    {
        return root.join("Prowlarr");
    }
    #[cfg(target_os = "windows")]
    {
        return root.join("Prowlarr.exe");
    }
    root.join("Prowlarr")
}

/// Downloads, verifies, and unpacks the pinned Prowlarr into data_dir/sidecars/prowlarr.
///
/// The download is staged as a .part file, hashed as the bytes arrive, and refused the first byte
/// past the pinned size, so an oversized or substituted response never reaches the disk in full.
/// Extraction goes to a staging directory that is renamed into place, so a partial pack is never
/// runnable.
///
/// # Errors
///
/// UnsupportedPlatform when this OS/arch is not pinned; the transport, size, checksum, archive,
/// and I/O variants for a download that failed; and Bundle when macOS refused to un-quarantine or
/// re-sign the app.
pub async fn install(data_dir: &Path, progress: &AtomicU64) -> Result<Installed, ProwlarrError> {
    let artifact = platform()?;
    progress.store(0, Ordering::Relaxed);
    let directory = data_dir.join("sidecars").join("prowlarr");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(ProwlarrError::Io)?;

    let root = directory.join(VERSION);
    let installed = Installed { root: root.clone() };
    if installed.is_ready() {
        progress.store(artifact.size, Ordering::Relaxed);
        return Ok(installed);
    }

    let staged = directory.join(format!("{VERSION}.part"));
    download(&artifact, &staged, progress).await?;

    let unpacking = directory.join(format!("{VERSION}.unpacking"));
    let _ = tokio::fs::remove_dir_all(&unpacking).await;
    tokio::fs::create_dir_all(&unpacking)
        .await
        .map_err(ProwlarrError::Io)?;
    extract(artifact.archive, staged.clone(), unpacking.clone()).await?;
    prepare_bundle(&unpacking)?;

    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::rename(&unpacking, &root)
        .await
        .map_err(ProwlarrError::Io)?;
    let _ = tokio::fs::remove_file(&staged).await;

    let installed = Installed { root };
    if !installed.is_ready() {
        return Err(ProwlarrError::MissingExecutable);
    }
    Ok(installed)
}

/// The pack already on disk under `data_dir`, if a complete one is there.
///
/// A runtime discovers this at start-up so a restart shows an installed pack as installed without
/// re-downloading anything; the install job is idempotent for the same reason.
#[must_use]
pub fn installed(data_dir: &Path) -> Option<Installed> {
    let root = data_dir.join("sidecars").join("prowlarr").join(VERSION);
    let installed = Installed { root };
    installed.is_ready().then_some(installed)
}

/// Streams the release to destination, rejecting the first byte past the pinned size and comparing
/// both the byte count and the SHA-256 before it can be extracted.
async fn download(
    artifact: &Artifact,
    destination: &Path,
    progress: &AtomicU64,
) -> Result<(), ProwlarrError> {
    use tokio::io::AsyncWriteExt as _;

    let client = client()?;
    let mut response = client
        .get(artifact.url)
        .send()
        .await
        .map_err(|_| ProwlarrError::Transport)?;
    if !response.status().is_success() {
        return Err(ProwlarrError::Transport);
    }

    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(ProwlarrError::Io)?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProwlarrError::Transport)?
    {
        total = total.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if total > artifact.size {
            return Err(ProwlarrError::SizeMismatch);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(ProwlarrError::Io)?;
        progress.store(total, Ordering::Relaxed);
    }
    file.flush().await.map_err(ProwlarrError::Io)?;

    if total != artifact.size {
        return Err(ProwlarrError::SizeMismatch);
    }
    if hex(&hasher.finalize()) != artifact.sha256 {
        return Err(ProwlarrError::ChecksumMismatch);
    }
    Ok(())
}

/// A client for the release request and the health poll.
fn client() -> Result<reqwest::Client, ProwlarrError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| ProwlarrError::Transport)
}

/// Unpacks an archive off the async runtime's threads.
async fn extract(
    archive: Archive,
    source: PathBuf,
    destination: PathBuf,
) -> Result<(), ProwlarrError> {
    tokio::task::spawn_blocking(move || match archive {
        Archive::Zip => extract_zip(&source, &destination),
        Archive::TarGz => extract_tar_gz(&source, &destination),
    })
    .await
    .map_err(|_| ProwlarrError::Archive)?
}

/// Unpacks a .zip, refusing any entry whose path escapes the destination.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn extract_zip(source: &Path, destination: &Path) -> Result<(), ProwlarrError> {
    let file = std::fs::File::open(source).map_err(ProwlarrError::Io)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| ProwlarrError::Archive)?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| ProwlarrError::Archive)?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(ProwlarrError::Archive);
        };
        let target = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(ProwlarrError::Io)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(ProwlarrError::Io)?;
        }
        let mut out = std::fs::File::create(&target).map_err(ProwlarrError::Io)?;
        std::io::copy(&mut entry, &mut out).map_err(ProwlarrError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

/// Never reached: a zip is named only on macOS and Windows, where the real function above is
/// compiled.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn extract_zip(_source: &Path, _destination: &Path) -> Result<(), ProwlarrError> {
    Err(ProwlarrError::UnsupportedPlatform)
}

/// Unpacks a .tar.gz. The tar crate refuses entries that escape the destination itself.
#[cfg(target_os = "linux")]
fn extract_tar_gz(source: &Path, destination: &Path) -> Result<(), ProwlarrError> {
    let file = std::fs::File::open(source).map_err(ProwlarrError::Io)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(destination)
        .map_err(|_| ProwlarrError::Archive)?;
    Ok(())
}

/// Never reached: a tar.gz is named only on Linux, where the real function above is compiled.
#[cfg(not(target_os = "linux"))]
fn extract_tar_gz(_source: &Path, _destination: &Path) -> Result<(), ProwlarrError> {
    Err(ProwlarrError::UnsupportedPlatform)
}

/// Runs the two commands Prowlarr's own macOS instructions name, so the extracted app runs without
/// the quarantine flag the download left on it. A no-op off macOS.
#[cfg(target_os = "macos")]
fn prepare_bundle(staging: &Path) -> Result<(), ProwlarrError> {
    let app = find_app(staging)?;
    let unquarantine = Command::new("xattr")
        .args(["-rd", "com.apple.quarantine"])
        .arg(&app)
        .status()
        .map_err(ProwlarrError::Io)?;
    if !unquarantine.success() {
        return Err(ProwlarrError::Bundle);
    }
    let resign = Command::new("codesign")
        .args(["--force", "--deep", "-s", "-"])
        .arg(&app)
        .status()
        .map_err(ProwlarrError::Io)?;
    if !resign.success() {
        return Err(ProwlarrError::Bundle);
    }
    Ok(())
}

/// Off macOS there is no quarantine flag and no app bundle to re-sign.
#[cfg(not(target_os = "macos"))]
fn prepare_bundle(_staging: &Path) -> Result<(), ProwlarrError> {
    Ok(())
}

/// The one .app directory an unpacked macOS pack must contain.
#[cfg(target_os = "macos")]
fn find_app(staging: &Path) -> Result<PathBuf, ProwlarrError> {
    for entry in std::fs::read_dir(staging).map_err(ProwlarrError::Io)? {
        let path = entry.map_err(ProwlarrError::Io)?.path();
        if path.extension().is_some_and(|extension| extension == "app") {
            return Ok(path);
        }
    }
    Err(ProwlarrError::MissingExecutable)
}

/// A running, managed Prowlarr.
///
/// The child is stopped when this is dropped or Supervisor::stop is called, so a session that ends
/// does not leave an indexer manager behind.
#[derive(Debug)]
pub struct Supervisor {
    child: Child,
    base_url: String,
}

impl Supervisor {
    /// Spawns the installed Prowlarr on loopback, waits for it to answer, and stores the endpoint
    /// and the key it generated under the names the indexer settings use.
    ///
    /// The key is read from Prowlarr's own config.xml after it is ready, because Prowlarr mints it
    /// on first run. It never appears in this function's error or return.
    ///
    /// # Errors
    ///
    /// MissingExecutable when the pack is not installed; NotReady when it does not answer within
    /// the timeout, with the child stopped before that is returned; and the config and keyring
    /// variants for a key that could not be read or stored.
    pub async fn start(
        installed: &Installed,
        store: Arc<dyn SecretStore>,
        data_dir: &Path,
    ) -> Result<Self, ProwlarrError> {
        let executable = installed.executable();
        if !executable.is_file() {
            return Err(ProwlarrError::MissingExecutable);
        }
        let config_dir = data_dir.join("prowlarr-data");
        tokio::fs::create_dir_all(&config_dir)
            .await
            .map_err(ProwlarrError::Io)?;

        let client = client()?;
        let port = read_config(&config_dir).map_or(DEFAULT_PORT, |config| config.port);
        // An instance left running by a previous process still answers here. Reusing the port
        // would put two Prowlarrs on one config directory, so refuse rather than spawn a second.
        if answers(&client, port).await {
            return Err(ProwlarrError::AlreadyRunning);
        }
        // The data-directory argument is read only in its equals form, -data=<dir>. With a
        // space Prowlarr ignores it and writes into the user's own default config directory,
        // so the supervisor would look for a config.xml that is never there. Verified against
        // a real 2.6.5.5623 run.
        let data_argument = format!("-data={}", config_dir.display());
        let mut child = Command::new(&executable)
            .arg(data_argument)
            .arg("-nobrowser")
            // Prowlarr defaults BindAddress to every interface. This override keeps the managed
            // instance on loopback (ADR 0024), and it is set on every spawn because the value is
            // not persisted into config.xml. The port is deliberately NOT overridden: an
            // environment port is not written to config.xml, and the persisted one is what the
            // supervisor reads back to build the URL.
            .env("PROWLARR__SERVER__BINDADDRESS", "127.0.0.1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(ProwlarrError::Io)?;

        // Every path after the spawn either succeeds or reaps the child. An orphaned Prowlarr is
        // a listener nobody stops and a port the next attempt cannot bind; a real run left one.
        match Self::finish_start(&client, &config_dir, &store, port).await {
            Ok(base_url) => Ok(Self { child, base_url }),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }

    /// The part of a start that can fail: wait for health, read the key it wrote, store it.
    async fn finish_start(
        client: &reqwest::Client,
        config_dir: &Path,
        store: &Arc<dyn SecretStore>,
        port: u16,
    ) -> Result<String, ProwlarrError> {
        if !wait_ready(client, port).await {
            return Err(ProwlarrError::NotReady);
        }
        let config = read_config_when_written(config_dir)
            .await
            .ok_or(ProwlarrError::Config)?;
        let base_url = format!("http://127.0.0.1:{}", config.port);
        let endpoint = base_url.clone();
        let key = config.api_key;
        let store = Arc::clone(store);
        tokio::task::spawn_blocking(move || {
            store.set(&endpoint_key(), &Secret::new(endpoint))?;
            store.set(&api_key_key(), &Secret::new(key))
        })
        .await
        .map_err(|_| ProwlarrError::Task)??;
        Ok(base_url)
    }

    /// The loopback endpoint the managed instance answers on.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Stops the child and waits for it, so no listener outlives the call.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Prowlarr's own configuration, as far as the app reads it. Never derived Debug: it holds a key.
struct Config {
    port: u16,
    api_key: String,
}

/// Reads config_dir/config.xml, or None when it is not there or not readable yet.
fn read_config(config_dir: &Path) -> Option<Config> {
    let text = std::fs::read_to_string(config_dir.join("config.xml")).ok()?;
    let mut reader = quick_xml::Reader::from_str(&text);
    let mut element = String::new();
    let mut port = None;
    let mut api_key = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                element = String::from_utf8_lossy(event.name().as_ref()).into_owned();
            }
            Ok(quick_xml::events::Event::Empty(_)) => element.clear(),
            // Clearing on the end tag is load-bearing: without it the whitespace between two
            // elements inherits the name of the one just closed, so the newline after
            // </ApiKey> overwrote the real key with an empty string and the whole read
            // answered None — a permanent start timeout. Verified with a standalone parser.
            Ok(quick_xml::events::Event::End(_)) => element.clear(),
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Ok(value) = text.xml10_content() {
                    let value = value.trim();
                    if !value.is_empty() {
                        match element.as_str() {
                            "Port" => port = value.parse::<u16>().ok(),
                            "ApiKey" => api_key = Some(value.to_owned()),
                            _ => {}
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    let api_key = api_key.filter(|key| !key.is_empty())?;
    Some(Config {
        port: port.unwrap_or(DEFAULT_PORT),
        api_key,
    })
}

/// Polls GET /ping until Prowlarr answers or the deadline passes.
async fn wait_ready(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/ping");
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(response) = client.get(&url).send().await
            && response.status().is_success()
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether something already answers Prowlarr's health check on this port.
async fn answers(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/ping");
    matches!(client.get(&url).send().await, Ok(response) if response.status().is_success())
}

/// Reads config.xml, retrying until Prowlarr has written its key or the deadline passes.
async fn read_config_when_written(config_dir: &Path) -> Option<Config> {
    let deadline = tokio::time::Instant::now() + CONFIG_TIMEOUT;
    loop {
        if let Some(config) = read_config(config_dir) {
            return Some(config);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Lowercase hex, for the one comparison the manifest needs.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
