//! Episode stills captured from the episode's own file.
//!
//! **The fallback that makes "downloaded, therefore it has a thumbnail" true.** A provider publishes
//! an episode's still some time after the payload exists — sometimes never — and a show whose season
//! was synced early keeps a NULL `still_path` with nothing to re-sync it. The file is already on
//! disk, so this module takes one frame from it when the download is imported and records that frame
//! as the episode's still.
//!
//! # It is the same store as a provider's still
//!
//! The blob goes into the image cache under [`ImageKey::for_captured_still`] and an `image_cache` row
//! is written beside it, so the card, the accounting, the eviction order, and the asset-protocol scope
//! are the ones a provider's still already uses. A rendered still is therefore not a second kind of
//! picture: the episode's columns carry `capture` as the owner and the episode id as the path, and the
//! read resolves those bytes exactly as it resolves TMDB's.
//!
//! # A capture never replaces a provider's still
//!
//! The SQL write is conditional on `still_path IS NULL` (`peregrine_storage::Database::set_episode_still`),
//! so a provider still that landed first is untouched, and an import that runs twice finds the column
//! filled and asks the capture port nothing. The one exception is a capture whose blob an eviction
//! removed: the reference is then stale rather than a still, so the next import regenerates the bytes
//! and leaves the columns as they are.
//!
//! # Nothing here blocks the runtime
//!
//! Frame extraction is a child process and the codec probe is another, both awaited; the cache's
//! synchronous write is the one filesystem call that is moved to a blocking thread. The port's own
//! contract is small enough that a test injects a fake which writes a fixture instead of running
//! ffmpeg (AGENTS R14), and a failed capture is a logged warning rather than a failed import: the
//! episode keeps its NULL still and the download that imported it is still a successful download.
//!
//! # The episodes that predate the fallback
//!
//! An import that ran before this module existed left its episode with a NULL still, and nothing
//! re-imports it. [`EpisodeStills::backfill`] is the startup pass that finds those episodes through
//! the library rows, takes one frame from the file that is already recorded, and records it exactly
//! as an import would. It runs after the runtime is ready, attempts at most a fixed number per
//! launch, and waits between attempts so a large library does not run ffmpeg flat out. An episode
//! whose file is missing, or whose capture failed, keeps its NULL still and is a warning that names
//! the episode id and nothing else; the next launch tries again.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use peregrine_domain::Clock;
use peregrine_domain::catalogue::CAPTURED_STILL_PROVIDER;
use peregrine_storage::{Database, ImageCacheEntry};
use peregrine_sync::{CachedImage, ImageCache, ImageKey, ImageVariant};
use tokio::process::Command;
use tokio::time::Duration;
use tracing::warn;

/// How long one extraction attempt may run before the child is killed.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

/// Where a frame is taken when the duration could not be probed.
const DEFAULT_SEEK_MILLIS: i64 = 120_000;

/// The earliest frame position considered, so a very short file is not sampled at zero.
const MIN_SEEK_MILLIS: i64 = 30_000;

/// The latest frame position considered, so a long film's tenth is not half an hour in.
const MAX_SEEK_MILLIS: i64 = 300_000;

/// The largest page the startup backfill asks storage for at once.
///
/// A page larger than the attempt budget would read candidates the pass cannot reach, and a page of
/// one would make the keyset walk a round trip per episode. Sixteen is neither.
const BACKFILL_PAGE: u32 = 16;

/// Where one frame of a file is taken from.
///
/// A tenth in is the rule the renderer already uses for a local poster (FLOWS, "a film becomes its
/// own poster"), and the clamp keeps a very short file and a very long one both sampled somewhere a
/// picture exists: a fixed few minutes would land past the end of a short file, and a bare tenth of a
/// three-hour film would land in a scene the card does not need.
#[must_use]
pub fn seek_millis(duration_ms: Option<i64>) -> i64 {
    duration_ms.map_or(DEFAULT_SEEK_MILLIS, |duration| {
        (duration / 10).clamp(MIN_SEEK_MILLIS, MAX_SEEK_MILLIS)
    })
}

/// Why a frame could not be captured.
///
/// Deliberately path-free: a log line about a failed capture must not name the film's file
/// (SECURITY.md T13), and the caller already knows which episode it asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    /// The capture tool could not be started.
    #[error("the frame capture tool is not available")]
    Unavailable,
    /// The capture ran longer than its budget.
    #[error("the frame capture timed out")]
    TimedOut,
    /// The tool ran but wrote no frame.
    #[error("the frame capture produced no frame")]
    Empty,
    /// The tool refused the file.
    #[error("the frame capture failed")]
    Failed,
}

/// Extracts one JPEG frame from a file.
///
/// A port rather than a function so a test can inject a fake that writes a fixture and never runs
/// ffmpeg (AGENTS R14). The real adapter is [`FfmpegCapture`]; the asynchronous shape is what keeps a
/// child process off the runtime's blocking threads.
pub trait FrameCapture: Send + Sync {
    /// Writes one JPEG frame of `source` to `destination`.
    fn capture<'a>(
        &'a self,
        source: &'a Path,
        destination: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), CaptureError>> + Send + 'a>>;
}

/// The ffmpeg this build runs, resolved exactly as the HLS transcode resolves it.
#[derive(Debug, Clone)]
pub struct FfmpegCapture {
    ffmpeg: PathBuf,
}

impl FfmpegCapture {
    /// Resolves ffmpeg through the HLS transcoder's own lookup, or answers `None` when no binary exists.
    ///
    /// The lookup shares the HLS transcoder's own `locate` rather than spelling the sidecar paths a
    /// second time. Its development fallback is the bare name `ffmpeg`, which is checked against
    /// `PATH` here so a host without one does not construct a capturer whose every attempt fails.
    #[must_use]
    pub fn resolve() -> Option<Self> {
        let located = crate::web::locate_ffmpeg();
        let ffmpeg = if located.components().count() > 1 || located.is_absolute() {
            located.is_file().then_some(located)?
        } else {
            let paths = std::env::var_os("PATH")?;
            std::env::split_paths(&paths)
                .map(|directory| directory.join(&located))
                .find(|candidate| candidate.is_file())?
        };
        Some(Self { ffmpeg })
    }
}

impl FrameCapture for FfmpegCapture {
    fn capture<'a>(
        &'a self,
        source: &'a Path,
        destination: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), CaptureError>> + Send + 'a>> {
        Box::pin(async move {
            let duration = crate::web::duration_ms(source).await;
            let seek = seek_millis(duration);
            // A position past the end of a file ffprobe could not measure makes ffmpeg exit without
            // a frame; starting over at the first frame is the one retry that can still answer.
            for position in [seek, 0] {
                run(&self.ffmpeg, source, destination, position).await?;
                if tokio::fs::metadata(destination)
                    .await
                    .is_ok_and(|file| file.is_file() && file.len() > 0)
                {
                    return Ok(());
                }
                let _ = tokio::fs::remove_file(destination).await;
            }
            Err(CaptureError::Empty)
        })
    }
}

/// Runs one ffmpeg invocation, restarting `-ss` at `seek_millis` in.
async fn run(
    ffmpeg: &Path,
    source: &Path,
    destination: &Path,
    seek_millis: i64,
) -> Result<(), CaptureError> {
    let mut command = Command::new(ffmpeg);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error")
        .arg("-y");
    if seek_millis > 0 {
        command
            .arg("-ss")
            .arg(format!("{:.3}", seek_millis as f64 / 1000.0));
    }
    command
        .arg("-i")
        .arg(source)
        .arg("-frames:v")
        .arg("1")
        .arg("-q:v")
        .arg("3")
        .arg("-f")
        .arg("image2")
        .arg(destination)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| CaptureError::Unavailable)?;
    match tokio::time::timeout(CAPTURE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(_)) => Err(CaptureError::Failed),
        Ok(Err(_)) => Err(CaptureError::Unavailable),
        Err(_) => {
            let _ = child.kill().await;
            Err(CaptureError::TimedOut)
        }
    }
}

/// Where the captured still of one episode is, or `None` when the cache holds none.
///
/// A filesystem `stat` rather than a row read, for the reason [`crate::artwork::cached_image`] gives:
/// a row whose blob eviction removed is a path the asset protocol refuses.
#[must_use]
pub fn cached(cache: &ImageCache, episode_id: i64) -> Option<PathBuf> {
    let key = ImageKey::for_captured_still(episode_id);
    cache.contains(&key).then(|| cache.disk_path(&key))
}

/// Why an episode's still could not be captured or recorded.
#[derive(Debug, thiserror::Error)]
pub enum StillError {
    /// The episode's still columns could not be read or written.
    #[error(transparent)]
    Storage(#[from] peregrine_storage::StorageError),
    /// The frame itself could not be extracted.
    #[error(transparent)]
    Capture(#[from] CaptureError),
    /// The extracted frame's bytes could not be read back.
    #[error("the captured frame could not be read")]
    Read,
    /// The frame could not be written into the image cache.
    #[error("the captured frame could not be stored")]
    Store(#[source] peregrine_sync::ImageCacheError),
    /// The blocking store could not be joined.
    #[error("the captured frame could not be stored")]
    Join,
}

/// The capture port, the cache, and the database, in the one object a completed import calls.
pub struct EpisodeStills {
    cache: Arc<ImageCache>,
    database: Arc<Database>,
    clock: Arc<dyn Clock + Send + Sync>,
    capture: Arc<dyn FrameCapture>,
    /// Keeps two concurrent captures in one process from sharing a scratch name without a random source.
    writers: AtomicU64,
}

impl EpisodeStills {
    /// Builds the service over its ports.
    #[must_use]
    pub fn new(
        cache: Arc<ImageCache>,
        database: Arc<Database>,
        clock: Arc<dyn Clock + Send + Sync>,
        capture: Arc<dyn FrameCapture>,
    ) -> Self {
        Self {
            cache,
            database,
            clock,
            capture,
            writers: AtomicU64::new(0),
        }
    }

    /// Captures and records a still for `episode_id`, unless one is already recorded.
    ///
    /// Answers whether a still was captured now. An episode that records a still is left alone
    /// without asking the port, which is the rule a provider's late still depends on. A stale capture
    /// whose blob an eviction removed is regenerated, because no still exists to protect there.
    ///
    /// # Errors
    ///
    /// [`StillError`] when the frame cannot be extracted, the episode cannot be read or written, or
    /// the cache cannot store the bytes. The caller treats every one of those as a warning: the
    /// import it is part of is still a successful import.
    pub async fn capture_if_missing(
        &self,
        episode_id: i64,
        source: &Path,
    ) -> Result<bool, StillError> {
        if let Some(recorded) = self.database.episode_still(episode_id).await? {
            let key = ImageKey::for_captured_still(episode_id);
            let stale = recorded.provider == CAPTURED_STILL_PROVIDER && !self.cache.contains(&key);
            if !stale {
                return Ok(false);
            }
        }
        // A payload the engine has not flushed, a file the user removed between the import and this
        // call, and a directory all answer the same way: nothing to capture, and no warning.
        if !tokio::fs::metadata(source)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            return Ok(false);
        }

        let scratch = self.scratch_path();
        let captured = match self.capture.capture(source, &scratch).await {
            Ok(()) => tokio::fs::read(&scratch)
                .await
                .map_err(|_| StillError::Read),
            Err(error) => Err(StillError::Capture(error)),
        };
        let _ = tokio::fs::remove_file(&scratch).await;
        let bytes = captured?;
        if bytes.is_empty() {
            return Err(StillError::Capture(CaptureError::Empty));
        }

        let key = ImageKey::for_captured_still(episode_id);
        let cache = Arc::clone(&self.cache);
        let stored = tokio::task::spawn_blocking(move || cache.store(&key, &bytes))
            .await
            .map_err(|_| StillError::Join)?
            .map_err(StillError::Store)?;
        let at = self.clock.now().as_millisecond();
        self.record(episode_id, &stored, at).await?;
        Ok(true)
    }

    /// Captures stills for episodes a previous import left without one.
    ///
    /// **The same rule as a fresh import, run over the rows that already exist.** The read only
    /// answers episodes whose `still_path` is NULL and whose media file is recorded and not marked
    /// missing; a provider still, a captured still, and an episode with no file are therefore never
    /// asked about. `capture_if_missing` repeats the check under the write, so a provider still that
    /// lands while this pass runs still wins.
    ///
    /// **Bounded work, not a library scan.** At most `at_most` attempts are made per call, and
    /// `between` is awaited after each attempt that consumed budget. A candidate whose file is gone
    /// answers `false` without asking the port, so a stale row costs a stat rather than a child
    /// process. Pages are keyset by episode id — the pass reads forward only and never uses OFFSET.
    ///
    /// A failed attempt is one warning naming the episode id and the path-free error; it counts
    /// against the budget because it may have run ffmpeg, and the next launch retries it.
    ///
    /// # Errors
    ///
    /// [`StillError::Storage`] when the candidate read fails. A failure to capture one episode is
    /// logged and counted rather than returned, because one damaged file must not stop the pass.
    pub async fn backfill(&self, at_most: usize, between: Duration) -> Result<usize, StillError> {
        let mut after = 0;
        let mut captured = 0;
        let mut attempted = 0;
        while attempted < at_most {
            let remaining = at_most - attempted;
            let limit = u32::try_from(remaining)
                .unwrap_or(u32::MAX)
                .min(BACKFILL_PAGE);
            let page = self.database.episodes_missing_still(after, limit).await?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.episode_id;
            for candidate in page {
                if attempted == at_most {
                    break;
                }
                let source = Path::new(candidate.path.as_str());
                let used_attempt = match self.capture_if_missing(candidate.episode_id, source).await
                {
                    Ok(true) => {
                        captured += 1;
                        true
                    }
                    Ok(false) => false,
                    Err(error) => {
                        warn!(
                            %error,
                            episode = candidate.episode_id,
                            "an episode still could not be captured while backfilling the library"
                        );
                        true
                    }
                };
                if used_attempt {
                    attempted += 1;
                    if attempted < at_most {
                        tokio::time::sleep(between).await;
                    }
                }
            }
        }
        Ok(captured)
    }

    /// Writes the cache row and then the episode's reference.
    ///
    /// The order is the one `image_cache` documents: the bytes are already published, the row records
    /// where, and the episode's columns are written last so a crash between the steps leaves bytes
    /// nothing reads rather than a reference to a file that is not there.
    async fn record(
        &self,
        episode_id: i64,
        stored: &CachedImage,
        at: i64,
    ) -> Result<(), StillError> {
        let entry = ImageCacheEntry {
            key: stored.key().as_str().to_owned(),
            provider: CAPTURED_STILL_PROVIDER.to_owned(),
            path: episode_id.to_string(),
            variant: ImageVariant::StillW300.as_str().to_owned(),
            disk_path: stored.disk_path().to_string_lossy().into_owned(),
            bytes: i64::try_from(stored.bytes()).unwrap_or(i64::MAX),
            // Unknown rather than invented: the ffmpeg invocation does not ask for a size and the
            // cache does not decode what it stores.
            width: None,
            height: None,
            created_at: at,
            last_used_at: at,
            hits: 0,
            // Not pinned: a captured frame is one import away from being regenerated, which is
            // exactly the kind of blob the artwork cap should be able to take back (PG-E6-10).
            pinned: false,
        };
        self.database.record_image(entry).await?;
        self.database
            .set_episode_still(
                episode_id,
                CAPTURED_STILL_PROVIDER,
                &episode_id.to_string(),
                at,
            )
            .await?;
        Ok(())
    }

    /// The temporary name ffmpeg writes to, which is removed as soon as the bytes are read.
    fn scratch_path(&self) -> PathBuf {
        let writer = self.writers.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "peregrine-still-{}-{writer}.jpg",
            std::process::id()
        ))
    }
}

impl std::fmt::Debug for EpisodeStills {
    /// Prints nothing about its halves: the cache's root and the database's location are paths that do
    /// not belong in a log line (SECURITY.md T13).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EpisodeStills")
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::AtomicUsize;

    use jiff::Timestamp;
    use peregrine_domain::{MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText};
    use peregrine_storage::{StubEpisode, StubSeason, StubTitle};
    use peregrine_testkit::{DEFAULT_EPOCH_MILLISECONDS, FakeClock};
    use tempfile::TempDir;

    use super::*;

    /// A frame extractor that writes a fixture and counts every call.
    #[derive(Debug, Default)]
    struct FakeCapture {
        calls: AtomicUsize,
    }

    impl FrameCapture for FakeCapture {
        fn capture<'a>(
            &'a self,
            _source: &'a Path,
            destination: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<(), CaptureError>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                tokio::fs::write(destination, b"fixture frame")
                    .await
                    .map_err(|_| CaptureError::Failed)
            })
        }
    }

    /// A show, a season, an empty cache, and the fake behind the port.
    struct StillHarness {
        stills: EpisodeStills,
        database: Arc<Database>,
        directory: TempDir,
        title_id: i64,
        season_id: i64,
    }

    impl StillHarness {
        async fn new(capture: Arc<FakeCapture>) -> Self {
            let directory = TempDir::new().expect("a temporary directory");
            let database = Arc::new(
                Database::open(directory.path().join("peregrine.sqlite"))
                    .await
                    .expect("the database opens"),
            );
            let at = DEFAULT_EPOCH_MILLISECONDS;
            let title_id = database
                .insert_stub_title(StubTitle {
                    kind: peregrine_domain::TitleKind::Show,
                    subtype: peregrine_domain::TitleSubtype::Standard,
                    name: "VisionQuest".to_owned(),
                    original_name: None,
                    sort_name: "VisionQuest".to_owned(),
                    at,
                })
                .await
                .expect("the show is inserted");
            let season_id = database
                .insert_stub_season(StubSeason {
                    title_id,
                    season_number: 1,
                    at,
                })
                .await
                .expect("the season is inserted");
            let cache = Arc::new(ImageCache::new(directory.path().join("images")));
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FakeClock::default());
            let stills = EpisodeStills::new(
                cache,
                Arc::clone(&database),
                clock,
                Arc::clone(&capture) as Arc<dyn FrameCapture>,
            );
            Self {
                stills,
                database,
                directory,
                title_id,
                season_id,
            }
        }

        /// Inserts one episode of the fixture season and answers its id.
        async fn episode(&self, number: u32) -> i64 {
            let at = DEFAULT_EPOCH_MILLISECONDS;
            self.database
                .insert_stub_episode(StubEpisode {
                    title_id: self.title_id,
                    season_id: self.season_id,
                    season_number: 1,
                    episode_number: number,
                    at,
                })
                .await
                .expect("the episode is inserted");
            self.database
                .episode_id_for(self.title_id, 1, number)
                .await
                .expect("the read answers")
                .expect("the episode is there")
        }

        /// Gives the episode one recorded file, writing it only when the test asks.
        async fn record_file(&self, episode_id: i64, name: &str, present: bool) -> PathBuf {
            let path = self.directory.path().join(name);
            if present {
                std::fs::write(&path, b"payload").expect("the fixture file");
            }
            let at = Timestamp::from_millisecond(DEFAULT_EPOCH_MILLISECONDS)
                .expect("the fixture instant is representable");
            let item = self
                .database
                .insert_media_item(MediaItem {
                    id: 0,
                    title_id: self.title_id,
                    episode_id: Some(episode_id),
                    state: MediaItemState::Available,
                    origin: MediaOrigin::Manual,
                    preferred_file_id: None,
                    quality_label: None,
                    created_at: at,
                    updated_at: at,
                })
                .await
                .expect("the item is inserted");
            self.database
                .insert_media_file(MediaFile {
                    id: 0,
                    media_item_id: item,
                    path: NonEmptyText::new(path.to_str().expect("the fixture path is UTF-8"))
                        .expect("the fixture path is not empty"),
                    root_id: None,
                    size_bytes: 7,
                    mtime_ms: at,
                    added_at: at,
                    missing_since: None,
                    container: None,
                    duration_ms: None,
                    video_codec: None,
                    video_profile: None,
                    width: None,
                    height: None,
                    bit_depth: None,
                    hdr_format: None,
                    frame_rate: None,
                    bitrate_kbps: None,
                    audio_codec: None,
                    audio_channels: None,
                    audio_layout: None,
                    audio_languages: None,
                    subtitle_format: None,
                    probe_version: None,
                    probe_json: None,
                    quality_label: None,
                    updated_at: at,
                })
                .await
                .expect("the file is inserted");
            path
        }
    }

    #[test]
    fn a_frame_is_taken_a_tenth_in_within_the_clamp() {
        assert_eq!(seek_millis(Some(22 * 60_000)), 132_000);
        assert_eq!(seek_millis(Some(45 * 60_000)), 270_000);
        assert_eq!(seek_millis(Some(60_000)), MIN_SEEK_MILLIS);
        assert_eq!(seek_millis(Some(3 * 60 * 60_000)), MAX_SEEK_MILLIS);
        assert_eq!(seek_millis(None), DEFAULT_SEEK_MILLIS);
    }

    #[test]
    fn a_blob_the_cache_holds_is_answered_with_its_path() {
        let root = std::env::temp_dir().join("peregrine-captured-still");
        let _ = std::fs::remove_dir_all(&root);
        let cache = ImageCache::new(&root);
        let key = ImageKey::for_captured_still(7);
        cache
            .store(&key, b"frame")
            .expect("the scratch is writable");

        assert_eq!(cached(&cache, 7), Some(cache.disk_path(&key)));
        assert_eq!(
            cached(&cache, 8),
            None,
            "one episode's key is not another's"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_episode_without_a_still_is_captured_from_its_recorded_file() {
        let capture = Arc::new(FakeCapture::default());
        let harness = StillHarness::new(Arc::clone(&capture)).await;
        let episode = harness.episode(3).await;
        harness
            .record_file(episode, "VisionQuest.S01E03.mkv", true)
            .await;

        let captured = harness
            .stills
            .backfill(8, Duration::ZERO)
            .await
            .expect("the pass answers");

        assert_eq!(captured, 1, "the one candidate is captured");
        assert_eq!(capture.calls.load(Ordering::Relaxed), 1);
        let still = harness
            .database
            .episode_still(episode)
            .await
            .expect("the read answers")
            .expect("the episode records a still");
        assert_eq!(still.provider, CAPTURED_STILL_PROVIDER);
        assert_eq!(still.path, episode.to_string());
    }

    #[tokio::test]
    async fn a_second_pass_does_not_recapture_an_episode_that_now_has_a_still() {
        let capture = Arc::new(FakeCapture::default());
        let harness = StillHarness::new(Arc::clone(&capture)).await;
        let episode = harness.episode(3).await;
        harness
            .record_file(episode, "VisionQuest.S01E03.mkv", true)
            .await;

        let first = harness
            .stills
            .backfill(8, Duration::ZERO)
            .await
            .expect("the first pass answers");
        let second = harness
            .stills
            .backfill(8, Duration::ZERO)
            .await
            .expect("the second pass answers");

        assert_eq!((first, second), (1, 0));
        assert_eq!(
            capture.calls.load(Ordering::Relaxed),
            1,
            "the recorded still is the candidate read absence"
        );
    }

    #[tokio::test]
    async fn an_episode_with_a_provider_still_is_not_a_candidate() {
        let capture = Arc::new(FakeCapture::default());
        let harness = StillHarness::new(Arc::clone(&capture)).await;
        let episode = harness.episode(4).await;
        harness
            .record_file(episode, "VisionQuest.S01E04.mkv", true)
            .await;
        harness
            .database
            .set_episode_still(
                episode,
                "tmdb",
                "/still-1-4.jpg",
                DEFAULT_EPOCH_MILLISECONDS,
            )
            .await
            .expect("the provider still is recorded");

        let captured = harness
            .stills
            .backfill(8, Duration::ZERO)
            .await
            .expect("the pass answers");

        assert_eq!(captured, 0);
        assert_eq!(
            capture.calls.load(Ordering::Relaxed),
            0,
            "a provider still is never replaced by a capture"
        );
        let still = harness
            .database
            .episode_still(episode)
            .await
            .expect("the read answers")
            .expect("the provider still stays");
        assert_eq!(still.provider, "tmdb");
        assert_eq!(still.path, "/still-1-4.jpg");
    }

    #[tokio::test]
    async fn an_episode_whose_file_is_missing_is_skipped_without_a_capture() {
        let capture = Arc::new(FakeCapture::default());
        let harness = StillHarness::new(Arc::clone(&capture)).await;
        let episode = harness.episode(3).await;
        let path = harness.record_file(episode, "gone.mkv", false).await;
        assert!(!path.exists(), "the fixture file is deliberately absent");

        let captured = harness
            .stills
            .backfill(8, Duration::ZERO)
            .await
            .expect("a missing file is not an error");

        assert_eq!(captured, 0);
        assert_eq!(capture.calls.load(Ordering::Relaxed), 0);
        assert!(
            harness
                .database
                .episode_still(episode)
                .await
                .expect("the read answers")
                .is_none(),
            "the episode keeps its NULL still"
        );
    }

    #[tokio::test]
    async fn the_pass_attempts_no_more_than_its_cap() {
        let capture = Arc::new(FakeCapture::default());
        let harness = StillHarness::new(Arc::clone(&capture)).await;
        let mut episodes = Vec::new();
        for number in 1..=3 {
            let episode = harness.episode(number).await;
            harness
                .record_file(episode, &format!("VisionQuest.S01E{number:02}.mkv"), true)
                .await;
            episodes.push(episode);
        }

        let captured = harness
            .stills
            .backfill(2, Duration::ZERO)
            .await
            .expect("the pass answers");

        assert_eq!(captured, 2);
        assert_eq!(capture.calls.load(Ordering::Relaxed), 2);
        let mut recorded = 0;
        for episode in episodes {
            if harness
                .database
                .episode_still(episode)
                .await
                .expect("the read answers")
                .is_some()
            {
                recorded += 1;
            }
        }
        assert_eq!(recorded, 2, "the cap leaves the rest for the next pass");
    }
}
