//! The Usenet engine port and its NNTP adapter.
//!
//! One job is one task. A pool sized by the providers' connection budgets pulls the selected files
//! in NZB order; each worker fetches its file's articles over its own connection, decodes them, and
//! writes the bytes at the offset each part header names. A part that is
//! missing or fails its checksum is counted rather than fatal: the par2 stage is what decides
//! whether the payload can be repaired. Pause, resume, remove, and statistics are levels, not a
//! second state machine.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use peregrine_domain::{FileSelection, SafePath, SafetyReport, Verdict};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{Mutex, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::nntp::{NntpConfig, NntpConnection, NntpError};
use crate::nzb::{NzbDocument, NzbFile};
use crate::yenc;

/// A provider the engine may use, in preference order.
#[derive(Clone)]
pub struct ServerConfig {
    /// The connection settings.
    pub nntp: NntpConfig,
    /// How many parallel connections this provider allows.
    pub connections: u32,
    /// Lower values are tried first.
    pub priority: i64,
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerConfig")
            .field("nntp", &self.nntp)
            .field("connections", &self.connections)
            .field("priority", &self.priority)
            .finish()
    }
}

/// Why an engine operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// The engine has been shut down.
    #[error("the Usenet engine is closed")]
    SessionClosed,
    /// The handle is not one this engine holds.
    #[error("the Usenet engine does not hold that job")]
    UnknownHandle,
    /// No enabled provider was configured.
    #[error("no Usenet provider is configured")]
    NoServer,
    /// The provider could not be reached or refused the account.
    #[error(transparent)]
    Transport(#[from] NntpError),
    /// A file could not be written.
    #[error("the payload could not be written")]
    Io,
}

/// Why an add was refused before it reached the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddRefused {
    /// The safety report did not allow the payload.
    #[error("the safety gate did not allow this Usenet payload")]
    NotAllowed,
}

/// One add request. The only constructor takes an allowed safety report.
pub struct AddNzbRequest {
    document: NzbDocument,
    save_path: SafePath,
    selection: FileSelection,
    servers: Vec<ServerConfig>,
}

impl AddNzbRequest {
    /// Builds an add from a report the gate produced.
    ///
    /// # Errors
    ///
    /// AddRefused when the report is not an allowed one.
    pub fn new(
        document: NzbDocument,
        save_path: SafePath,
        selection: FileSelection,
        servers: Vec<ServerConfig>,
        report: &SafetyReport,
    ) -> Result<Self, AddRefused> {
        if report.verdict() != Verdict::Allowed {
            return Err(AddRefused::NotAllowed);
        }
        Ok(Self {
            document,
            save_path,
            selection,
            servers,
        })
    }

    /// The parsed post this add carries.
    #[must_use]
    pub const fn document(&self) -> &NzbDocument {
        &self.document
    }

    /// The destination the engine may write under.
    #[must_use]
    pub const fn save_path(&self) -> &SafePath {
        &self.save_path
    }

    /// The files the engine is to write.
    #[must_use]
    pub const fn selection(&self) -> &FileSelection {
        &self.selection
    }

    /// The providers this add may use.
    #[must_use]
    pub fn servers(&self) -> &[ServerConfig] {
        &self.servers
    }
}

/// A handle to one job in one engine session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsenetHandle(u64);

impl UsenetHandle {
    /// Wraps the session's identifier.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The identifier, for a log line.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One job's counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsenetStats {
    /// Payload bytes written so far.
    pub downloaded_bytes: u64,
    /// The payload's whole size.
    pub total_bytes: u64,
    /// Articles decoded.
    pub articles_done: u32,
    /// Articles the post lists.
    pub articles_total: u32,
    /// Articles the provider did not hold or that failed their checksum.
    pub articles_missing: u32,
    /// The most recent rate, in bytes per second.
    pub rate_bps: u64,
    /// How many connections the job is using.
    pub connections: u32,
    /// Seconds left, when a rate exists.
    pub eta_seconds: Option<u64>,
    /// Whether every article was attempted. A complete payload may still be missing articles
    /// that par2 has to repair.
    pub complete: bool,
    /// A terminal error, when the task stopped.
    pub error: Option<String>,
}

impl UsenetStats {
    /// Completion in permille, where 1000 is the whole payload.
    #[must_use]
    pub fn progress_permille(&self) -> i64 {
        if self.total_bytes == 0 {
            return 0;
        }
        let permille = self.downloaded_bytes.saturating_mul(1_000) / self.total_bytes;
        i64::try_from(permille.min(1_000)).unwrap_or(1_000)
    }
}

/// The port the acquisition service talks to.
pub trait UsenetEngine: Send + Sync {
    /// Names the engine and the version this build runs.
    fn descriptor(&self) -> (String, String);

    /// Starts one job and answers its handle.
    fn add(&self, request: AddNzbRequest) -> EngineFuture<'_, UsenetHandle>;

    /// Returns one job's counters.
    fn stats(&self, handle: UsenetHandle) -> EngineFuture<'_, UsenetStats>;

    /// Stops a job's workers, keeping what they wrote.
    fn pause(&self, handle: UsenetHandle) -> EngineFuture<'_, ()>;

    /// Lets a paused job continue.
    fn resume(&self, handle: UsenetHandle) -> EngineFuture<'_, ()>;

    /// Stops and forgets a job, keeping the files on disk.
    fn remove(&self, handle: UsenetHandle) -> EngineFuture<'_, ()>;

    /// Stops every job and refuses later operations.
    fn shutdown(&self) -> EngineFuture<'_, ()>;
}

/// A boxed engine future.
pub type EngineFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, EngineError>> + Send + 'a>>;

struct Job {
    stats: Arc<Mutex<UsenetStats>>,
    paused: watch::Sender<bool>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// The NNTP adapter.
pub struct NntpEngine {
    handles: Mutex<HashMap<u64, Job>>,
    next: AtomicU64,
    closed: std::sync::atomic::AtomicBool,
}

impl Default for NntpEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl NntpEngine {
    /// Builds an empty session.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handles: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    async fn job(&self, handle: UsenetHandle) -> Result<JobState, EngineError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(EngineError::SessionClosed);
        }
        let handles = self.handles.lock().await;
        let job = handles.get(&handle.0).ok_or(EngineError::UnknownHandle)?;
        Ok(JobState {
            stats: Arc::clone(&job.stats),
            paused: job.paused.clone(),
        })
    }
}

struct JobState {
    stats: Arc<Mutex<UsenetStats>>,
    paused: watch::Sender<bool>,
}

impl UsenetEngine for NntpEngine {
    fn descriptor(&self) -> (String, String) {
        ("nntp".to_owned(), env!("CARGO_PKG_VERSION").to_owned())
    }

    fn add(&self, request: AddNzbRequest) -> EngineFuture<'_, UsenetHandle> {
        Box::pin(async move {
            if self.closed.load(Ordering::SeqCst) {
                return Err(EngineError::SessionClosed);
            }
            if request.servers.is_empty() {
                return Err(EngineError::NoServer);
            }
            let handle = UsenetHandle::new(self.next.fetch_add(1, Ordering::SeqCst));
            let stats = Arc::new(Mutex::new(initial_stats(&request)));
            let paused = watch::channel(false);
            let cancel = CancellationToken::new();
            let task = tokio::spawn(run(request, Arc::clone(&stats), paused.1, cancel.clone()));
            self.handles.lock().await.insert(
                handle.0,
                Job {
                    stats,
                    paused: paused.0,
                    cancel,
                    task,
                },
            );
            Ok(handle)
        })
    }

    fn stats(&self, handle: UsenetHandle) -> EngineFuture<'_, UsenetStats> {
        Box::pin(async move {
            let job = self.job(handle).await?;
            Ok(job.stats.lock().await.clone())
        })
    }

    fn pause(&self, handle: UsenetHandle) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            let job = self.job(handle).await?;
            let _ = job.paused.send(true);
            Ok(())
        })
    }

    fn resume(&self, handle: UsenetHandle) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            let job = self.job(handle).await?;
            let _ = job.paused.send(false);
            Ok(())
        })
    }

    fn remove(&self, handle: UsenetHandle) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            if self.closed.load(Ordering::SeqCst) {
                return Err(EngineError::SessionClosed);
            }
            if let Some(job) = self.handles.lock().await.remove(&handle.0) {
                job.cancel.cancel();
                job.task.abort();
            }
            Ok(())
        })
    }

    fn shutdown(&self) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            self.closed.store(true, Ordering::SeqCst);
            let jobs = std::mem::take(&mut *self.handles.lock().await);
            for job in jobs.values() {
                job.cancel.cancel();
                job.task.abort();
            }
            Ok(())
        })
    }
}

fn initial_stats(request: &AddNzbRequest) -> UsenetStats {
    let files = request.document.files();
    let total_bytes = files
        .iter()
        .enumerate()
        .filter(|(index, _)| request.selection.contains(*index))
        .fold(0u64, |total, (_, file)| total.saturating_add(file.bytes()));
    let articles_total = files
        .iter()
        .enumerate()
        .filter(|(index, _)| request.selection.contains(*index))
        .map(|(_, file)| file.segments.len())
        .sum::<usize>();
    UsenetStats {
        downloaded_bytes: 0,
        total_bytes,
        articles_done: 0,
        articles_total: u32::try_from(articles_total).unwrap_or(u32::MAX),
        articles_missing: 0,
        rate_bps: 0,
        connections: 1,
        eta_seconds: None,
        complete: false,
        error: None,
    }
}

#[allow(clippy::too_many_lines)] // One linear job; splitting the per-file walk hides the order.
async fn run(
    request: AddNzbRequest,
    stats: Arc<Mutex<UsenetStats>>,
    paused: watch::Receiver<bool>,
    cancel: CancellationToken,
) {
    let mut servers = request.servers.clone();
    servers.sort_by_key(|server| server.priority);
    if servers.is_empty() {
        fail(&stats, "no Usenet provider is configured").await;
        return;
    }
    let slots = servers
        .iter()
        .map(|server| server.connections.max(1))
        .max()
        .unwrap_or(1);
    let mut groups: Vec<Vec<NntpConnection>> = Vec::new();
    for _ in 0..slots {
        let mut group = Vec::new();
        for server in &servers {
            if let Ok(connection) = NntpConnection::connect(&server.nntp).await {
                group.push(connection);
            }
        }
        if !group.is_empty() {
            groups.push(group);
        }
    }
    if groups.is_empty() {
        fail(&stats, "no Usenet provider could be reached").await;
        return;
    }
    let connections = groups.iter().map(Vec::len).sum::<usize>();
    stats.lock().await.connections = u32::try_from(connections).unwrap_or(u32::MAX);
    let server_list = Arc::new(servers);

    let files: Vec<NzbFile> = request
        .document
        .files()
        .iter()
        .enumerate()
        .filter(|(index, _)| request.selection.contains(*index))
        .map(|(_, file)| file.clone())
        .collect();
    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let mut workers = JoinSet::new();
    for group in groups {
        let files = Arc::clone(&files);
        let next = Arc::clone(&next);
        let failed = Arc::clone(&failed);
        let stats = Arc::clone(&stats);
        let cancel = cancel.clone();
        let mut paused = paused.clone();
        let save_path = request.save_path.clone();
        let servers = Arc::clone(&server_list);
        workers.spawn(async move {
            let mut group = group;
            loop {
                if cancel.is_cancelled() || failed.load(Ordering::SeqCst) {
                    break;
                }
                let position = next.fetch_add(1, Ordering::SeqCst);
                let Some(file) = files.get(position) else {
                    break;
                };
                if download_file(
                    file,
                    &mut group,
                    &servers,
                    &save_path,
                    &stats,
                    &mut paused,
                    &cancel,
                )
                .await
                .is_err()
                {
                    failed.store(true, Ordering::SeqCst);
                    fail(&stats, "the payload could not be written").await;
                    break;
                }
            }
            for connection in group {
                connection.quit().await;
            }
        });
    }
    while let Some(joined) = workers.join_next().await {
        if joined.is_err() {
            failed.store(true, Ordering::SeqCst);
            fail(&stats, "a Usenet worker stopped unexpectedly").await;
        }
    }
    if cancel.is_cancelled() || failed.load(Ordering::SeqCst) {
        return;
    }
    {
        let mut stats = stats.lock().await;
        stats.downloaded_bytes = stats.total_bytes;
        stats.complete = true;
    }
}

#[allow(clippy::too_many_arguments)] // The worker's whole shared state, named rather than hidden in a struct.
async fn download_file(
    file: &NzbFile,
    connections: &mut Vec<NntpConnection>,
    servers: &[ServerConfig],
    save_path: &SafePath,
    stats: &Arc<Mutex<UsenetStats>>,
    paused: &mut watch::Receiver<bool>,
    cancel: &CancellationToken,
) -> Result<(), ()> {
    let path = save_path.as_path().join(file.name.as_str());
    let mut writable = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .await
        .map_err(|_| ())?;
    let mut cursor = 0u64;
    for segment in &file.segments {
        if cancel.is_cancelled() {
            return Err(());
        }
        while *paused.borrow() {
            if paused.changed().await.is_err() {
                return Err(());
            }
        }
        let started = std::time::Instant::now();
        let (mut body, mut transport) = fetch_article(connections, &segment.message_id).await;
        if body.is_none() && transport {
            *connections = reconnect(servers).await;
            if !connections.is_empty() {
                (body, transport) = fetch_article(connections, &segment.message_id).await;
                let _ = transport;
            }
        }
        let Some(body) = body else {
            mark_missing(stats).await;
            continue;
        };
        let Ok(article) = yenc::decode(&body) else {
            mark_missing(stats).await;
            continue;
        };
        if let Some(size) = article.size {
            let _ = writable.set_len(size).await;
        }
        let offset = article
            .begin
            .map_or(cursor, |begin| begin.saturating_sub(1));
        if writable
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .is_err()
            || writable.write_all(&article.data).await.is_err()
        {
            return Err(());
        }
        cursor = offset.saturating_add(u64::try_from(article.data.len()).unwrap_or(0));
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        update(stats, article.data.len(), elapsed).await;
    }
    Ok(())
}

async fn fetch_article(
    connections: &mut [NntpConnection],
    message_id: &str,
) -> (Option<Vec<u8>>, bool) {
    let mut transport = false;
    for connection in connections.iter_mut() {
        match connection.body(message_id).await {
            Ok(Some(found)) => return (Some(found), transport),
            Ok(None) => {}
            Err(_) => transport = true,
        }
    }
    (None, transport)
}

async fn reconnect(servers: &[ServerConfig]) -> Vec<NntpConnection> {
    let mut connections = Vec::new();
    for server in servers {
        if let Ok(connection) = NntpConnection::connect(&server.nntp).await {
            connections.push(connection);
        }
    }
    connections
}

async fn update(stats: &Arc<Mutex<UsenetStats>>, bytes: usize, seconds: f64) {
    let mut stats = stats.lock().await;
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    stats.downloaded_bytes = stats.downloaded_bytes.saturating_add(bytes);
    stats.articles_done = stats.articles_done.saturating_add(1);
    stats.rate_bps = (bytes as f64 / seconds) as u64;
    let remaining = stats.total_bytes.saturating_sub(stats.downloaded_bytes);
    stats.eta_seconds = (stats.rate_bps > 0).then_some(remaining / stats.rate_bps);
    debug!(
        downloaded = stats.downloaded_bytes,
        total = stats.total_bytes,
        "a Usenet article was written"
    );
}

async fn mark_missing(stats: &Arc<Mutex<UsenetStats>>) {
    let mut stats = stats.lock().await;
    stats.articles_missing = stats.articles_missing.saturating_add(1);
    stats.articles_done = stats.articles_done.saturating_add(1);
}

async fn fail(stats: &Arc<Mutex<UsenetStats>>, error: &str) {
    let mut stats = stats.lock().await;
    stats.error = Some(error.to_owned());
}
#[cfg(test)]
mod tests {
    use super::*;
    use peregrine_domain::{InspectionLimits, OptIn, PathGuard, PayloadInspector};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    fn encoded(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for byte in data {
            let value = byte.wrapping_add(42);
            if matches!(value, 0 | 10 | 13 | 61) {
                out.push(b'=');
                out.push(value.wrapping_add(64));
            } else {
                out.push(value);
            }
        }
        out
    }

    fn crc(data: &[u8]) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(data);
        hasher.finalize()
    }

    fn article(data: &[u8], begin: usize, end: usize) -> Vec<u8> {
        let mut out =
            format!("=ybegin line=128 size=10 name=file.mkv\r\n=ypart begin={begin} end={end}\r\n")
                .into_bytes();
        out.extend_from_slice(&encoded(data));
        out.extend_from_slice(
            format!("\r\n=yend size=10 part=1 pcrc32={:08x}\r\n.\r\n", crc(data)).as_bytes(),
        );
        out
    }

    async fn fixture() -> (u16, tokio::task::JoinHandle<()>) {
        fixture_after(std::time::Duration::ZERO).await
    }

    async fn fixture_after(delay: std::time::Duration) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepts");
            let mut stream = BufReader::new(stream);
            stream.get_mut().write_all(b"200 ready\r\n").await.unwrap();
            let mut line = String::new();
            loop {
                line.clear();
                if stream.read_line(&mut line).await.unwrap() == 0 {
                    return;
                }
                let command = line.trim_end();
                if command.starts_with("BODY ") && !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if command == "BODY <one@test>" {
                    stream
                        .get_mut()
                        .write_all(b"222 0 <one@test>\r\n")
                        .await
                        .unwrap();
                    stream
                        .get_mut()
                        .write_all(&article(b"hello", 1, 5))
                        .await
                        .unwrap();
                } else if command == "BODY <two@test>" {
                    stream
                        .get_mut()
                        .write_all(b"222 0 <two@test>\r\n")
                        .await
                        .unwrap();
                    stream
                        .get_mut()
                        .write_all(&article(b"world", 6, 10))
                        .await
                        .unwrap();
                } else if command == "QUIT" {
                    stream.get_mut().write_all(b"205 bye\r\n").await.unwrap();
                    return;
                } else {
                    stream
                        .get_mut()
                        .write_all(b"430 missing\r\n")
                        .await
                        .unwrap();
                }
            }
        });
        (port, task)
    }

    #[tokio::test]
    async fn a_job_fetches_two_parts_and_writes_them_at_their_offsets() {
        let directory = tempfile::TempDir::new().unwrap();
        let nzb = "<nzb><file subject=\"file.mkv\" date=\"0\" poster=\"p\">\
             <segments><segment bytes=\"10\" number=\"1\">&lt;one@test&gt;</segment>\
             <segment bytes=\"10\" number=\"2\">&lt;two@test&gt;</segment></segments>\
             </file></nzb>"
            .to_owned();
        let document = NzbDocument::parse(nzb.as_bytes()).expect("parses");
        let guard = PathGuard::new(directory.path()).expect("a root");
        let report = PayloadInspector::new(InspectionLimits::new(100, u64::MAX)).inspect(
            &document.description(),
            &guard,
            OptIn::NotGranted,
        );
        assert_eq!(report.verdict(), Verdict::Allowed);
        let save_path = guard.resolve(directory.path()).expect("a path");
        let (port, task) = fixture().await;
        let request = AddNzbRequest::new(
            document,
            save_path,
            FileSelection::new([0]),
            vec![ServerConfig {
                nntp: NntpConfig {
                    host: "127.0.0.1".to_owned(),
                    port,
                    tls: false,
                    username: String::new(),
                    password: None,
                },
                connections: 1,
                priority: 1,
            }],
            &report,
        )
        .expect("an allowed report");

        let engine = NntpEngine::new();
        let handle = engine.add(request).await.expect("starts");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let stats = engine.stats(handle).await.unwrap();
            if stats.complete || stats.error.is_some() {
                assert!(stats.error.is_none(), "{:?}", stats.error);
                assert_eq!(stats.articles_missing, 0);
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the job never finished"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let written = tokio::fs::read(directory.path().join("file.mkv"))
            .await
            .expect("the payload is written");
        assert_eq!(written, b"helloworld");
        engine.shutdown().await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_article_is_counted_and_the_rest_is_kept() {
        let (port, task) = fixture().await;
        let directory = tempfile::TempDir::new().unwrap();
        let guard = PathGuard::new(directory.path()).expect("a root");
        let nzb = "<nzb><file subject=\"file.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   <segment bytes=\"5\" number=\"2\">&lt;missing@test&gt;</segment></segments>\
                   </file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("parses");
        let report = PayloadInspector::new(InspectionLimits::new(100, u64::MAX)).inspect(
            &document.description(),
            &guard,
            OptIn::NotGranted,
        );
        let save_path = guard.resolve(directory.path()).expect("resolves");
        let engine = NntpEngine::new();
        let handle = engine
            .add(
                AddNzbRequest::new(
                    document,
                    save_path,
                    FileSelection::new([0]),
                    vec![ServerConfig {
                        nntp: NntpConfig {
                            host: "127.0.0.1".to_owned(),
                            port,
                            tls: false,
                            username: String::new(),
                            password: None,
                        },
                        connections: 1,
                        priority: 1,
                    }],
                    &report,
                )
                .expect("an allowed report"),
            )
            .await
            .expect("starts");
        let stats = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let stats = engine.stats(handle).await.expect("stats");
                if stats.complete || stats.error.is_some() {
                    break stats;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job finishes");
        assert_eq!(stats.articles_missing, 1);
        assert!(
            stats.complete,
            "a repairable set still completes the transfer"
        );
        engine.remove(handle).await.expect("remove");
        task.abort();
    }

    #[tokio::test]
    async fn pause_stops_the_workers_and_resume_finishes_the_post() {
        let (port, task) = fixture_after(std::time::Duration::from_millis(100)).await;
        let directory = tempfile::TempDir::new().unwrap();
        let guard = PathGuard::new(directory.path()).expect("a root");
        let nzb = "<nzb><file subject=\"file.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   <segment bytes=\"5\" number=\"2\">&lt;two@test&gt;</segment></segments>\
                   </file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("parses");
        let report = PayloadInspector::new(InspectionLimits::new(100, u64::MAX)).inspect(
            &document.description(),
            &guard,
            OptIn::NotGranted,
        );
        let save_path = guard.resolve(directory.path()).expect("resolves");
        let engine = NntpEngine::new();
        let handle = engine
            .add(
                AddNzbRequest::new(
                    document,
                    save_path,
                    FileSelection::new([0]),
                    vec![ServerConfig {
                        nntp: NntpConfig {
                            host: "127.0.0.1".to_owned(),
                            port,
                            tls: false,
                            username: String::new(),
                            password: None,
                        },
                        connections: 1,
                        priority: 1,
                    }],
                    &report,
                )
                .expect("an allowed report"),
            )
            .await
            .expect("starts");
        engine.pause(handle).await.expect("pause");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(
            engine.stats(handle).await.expect("stats").downloaded_bytes,
            0
        );
        engine.resume(handle).await.expect("resume");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let stats = engine.stats(handle).await.expect("stats");
                if stats.complete || stats.error.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job finishes");
        let written = tokio::fs::read(directory.path().join("file.mkv"))
            .await
            .expect("the payload");
        assert_eq!(written, b"helloworld");
        engine.remove(handle).await.expect("remove");
        task.abort();
    }

    #[tokio::test]
    async fn removing_a_job_cancels_it_and_forgets_the_handle() {
        let (port, task) = fixture_after(std::time::Duration::from_millis(100)).await;
        let directory = tempfile::TempDir::new().unwrap();
        let guard = PathGuard::new(directory.path()).expect("a root");
        let nzb = "<nzb><file subject=\"file.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   </segments></file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("parses");
        let report = PayloadInspector::new(InspectionLimits::new(100, u64::MAX)).inspect(
            &document.description(),
            &guard,
            OptIn::NotGranted,
        );
        let save_path = guard.resolve(directory.path()).expect("resolves");
        let engine = NntpEngine::new();
        let handle = engine
            .add(
                AddNzbRequest::new(
                    document,
                    save_path,
                    FileSelection::new([0]),
                    vec![ServerConfig {
                        nntp: NntpConfig {
                            host: "127.0.0.1".to_owned(),
                            port,
                            tls: false,
                            username: String::new(),
                            password: None,
                        },
                        connections: 1,
                        priority: 1,
                    }],
                    &report,
                )
                .expect("an allowed report"),
            )
            .await
            .expect("starts");
        engine.remove(handle).await.expect("remove");
        assert!(matches!(
            engine.stats(handle).await,
            Err(EngineError::UnknownHandle)
        ));
        task.abort();
    }

    async fn fixture_two_connections() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let task = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("accepts");
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    stream.get_mut().write_all(b"200 ready\r\n").await.unwrap();
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if stream.read_line(&mut line).await.unwrap() == 0 {
                            return;
                        }
                        match line.trim_end() {
                            "BODY <one@test>" => {
                                stream
                                    .get_mut()
                                    .write_all(b"222 0 <one@test>\r\n")
                                    .await
                                    .unwrap();
                                stream
                                    .get_mut()
                                    .write_all(&article(b"hello", 1, 5))
                                    .await
                                    .unwrap();
                            }
                            "QUIT" => {
                                stream.get_mut().write_all(b"205 bye\r\n").await.unwrap();
                                return;
                            }
                            _ => {
                                stream
                                    .get_mut()
                                    .write_all(b"430 missing\r\n")
                                    .await
                                    .unwrap();
                            }
                        }
                    }
                });
            }
        });
        (port, task)
    }

    #[tokio::test]
    async fn two_files_are_fetched_over_two_connections() {
        let (port, task) = fixture_two_connections().await;
        let directory = tempfile::TempDir::new().unwrap();
        let guard = PathGuard::new(directory.path()).expect("a root");
        let nzb = "<nzb><file subject=\"one.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment></segments>\
                   </file><file subject=\"two.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment></segments>\
                   </file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("parses");
        let report = PayloadInspector::new(InspectionLimits::new(100, u64::MAX)).inspect(
            &document.description(),
            &guard,
            OptIn::NotGranted,
        );
        let save_path = guard.resolve(directory.path()).expect("resolves");
        let engine = NntpEngine::new();
        let handle = engine
            .add(
                AddNzbRequest::new(
                    document,
                    save_path,
                    FileSelection::new([0, 1]),
                    vec![ServerConfig {
                        nntp: NntpConfig {
                            host: "127.0.0.1".to_owned(),
                            port,
                            tls: false,
                            username: String::new(),
                            password: None,
                        },
                        connections: 2,
                        priority: 1,
                    }],
                    &report,
                )
                .expect("an allowed report"),
            )
            .await
            .expect("starts");
        let stats = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let stats = engine.stats(handle).await.expect("stats");
                if stats.complete || stats.error.is_some() {
                    break stats;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job finishes");
        assert_eq!(stats.connections, 2);
        assert!(stats.complete);
        assert!(
            tokio::fs::read(directory.path().join("one.mkv"))
                .await
                .unwrap()
                .starts_with(b"hello")
        );
        assert!(
            tokio::fs::read(directory.path().join("two.mkv"))
                .await
                .unwrap()
                .starts_with(b"hello")
        );
        engine.remove(handle).await.expect("remove");
        task.abort();
    }
}
