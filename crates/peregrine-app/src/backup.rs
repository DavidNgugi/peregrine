//! Creating a backup and staging a restore for the next launch.
//!
//! A backup is the database copied with `VACUUM INTO` and the process-local tables removed
//! (see `peregrine-storage::backup`). That makes it a complete answer for the household's own
//! data - profiles, wishlists, watch progress, settings, the catalogue, and the library index -
//! and an honest one for what it leaves out: downloaded files are not in the database, and
//! credentials live in the OS keychain (ADR 0033).
//!
//! A restore is staged rather than applied under the running process. The live database has a
//! writer task and a reader pool, and swapping the file beneath them is not something SQLite
//! offers; the next launch opens the restored file before any connection exists. The staged
//! file is `pending.sqlite` under the data directory's `restore/` folder, and the database it
//! replaces is copied to `previous.sqlite` first. That copy is the way back if the restored
//! file cannot be opened, and it is what the rollback path puts back.

use std::path::{Path, PathBuf};

use peregrine_storage::{BackupInfo, Database, StorageError};
use tokio::fs;
use tracing::{info, warn};

use crate::runtime::RuntimeError;

/// The folder under the data directory that holds a staged restore and its safety copy.
const RESTORE_DIRECTORY: &str = "restore";

/// The staged file, which is complete before it gets this name.
const PENDING: &str = "pending.sqlite";

/// The database the restore replaced, kept until the next restore.
const PREVIOUS: &str = "previous.sqlite";

/// A restored database that could not be opened, kept for diagnosis.
const FAILED: &str = "failed.sqlite";

/// What a staged file is called while it is being copied.
const PARTIAL: &str = "pending.sqlite.partial";

/// The steps of staging a restore, for the bar a person watches.
///
/// The same shape as the storage layer's backup phases: a share of the whole per step, not a
/// byte count SQLite does not report. The last step is emitted before the app restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePhase {
    /// An exact copy of the database being replaced is being written.
    Snapshotting,
    /// The chosen file is being copied into the staging folder.
    Staging,
    /// The staged file is complete; the restart is next.
    Staged,
}

impl RestorePhase {
    /// The stable spelling an event payload carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Snapshotting => "snapshotting",
            Self::Staging => "staging",
            Self::Staged => "staged",
        }
    }

    /// How far through the whole the phase starts, in permille.
    #[must_use]
    pub const fn permille(self) -> u16 {
        match self {
            Self::Snapshotting => 100,
            Self::Staging => 600,
            Self::Staged => 1_000,
        }
    }
}

/// The name a backup gets, from the instant the runtime's clock reports.
///
/// UTC and a fixed-width stamp: the file is portable, and two backups taken in the same second
/// get distinct names from [`free_destination`] rather than from a second clock read.
#[must_use]
pub fn backup_file_name(at_millis: i64) -> String {
    let stamp = jiff::Timestamp::from_millisecond(at_millis).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let text = stamp
        .to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%Y-%m-%d-%H%M%S")
        .to_string();
    format!("peregrine-backup-{text}.sqlite")
}

/// The first name in `directory` not already taken by `name`.
///
/// A second suffix rather than a refusal: a person who presses Back up twice in one second
/// should get two files, not an error about a name they did not see.
pub(crate) async fn free_destination(
    directory: &Path,
    name: &str,
) -> Result<PathBuf, StorageError> {
    let stem = name.strip_suffix(".sqlite").unwrap_or(name);
    for sequence in 1..=100 {
        let candidate = if sequence == 1 {
            directory.join(name)
        } else {
            directory.join(format!("{stem}-{sequence}.sqlite"))
        };
        if !fs::try_exists(&candidate).await.unwrap_or(false) {
            return Ok(candidate);
        }
    }
    Err(StorageError::Backup {
        path: directory.to_path_buf(),
        source: Box::new(std::io::Error::other(
            "no free backup name in that folder after 100 attempts",
        )),
    })
}

/// Validates `source`, copies it aside, and leaves it for the next launch.
///
/// Nothing in the running process is changed except the `restore/` folder: the live database
/// keeps serving reads until the app restarts, which is what makes a failed validation leave
/// the installation exactly as it was.
pub(crate) async fn stage_restore(
    database: &Database,
    data_directory: &Path,
    database_path: &Path,
    source: &Path,
    progress: &(dyn Fn(RestorePhase) + Send + Sync),
) -> Result<BackupInfo, RuntimeError> {
    let source = fs::canonicalize(source)
        .await
        .map_err(|_| RuntimeError::InvalidRequest {
            detail: "That backup could not be found.".to_owned(),
        })?;
    if !fs::metadata(&source)
        .await
        .map_err(|_| RuntimeError::InvalidRequest {
            detail: "That backup could not be read.".to_owned(),
        })?
        .is_file()
    {
        return Err(RuntimeError::InvalidRequest {
            detail: "That backup is not a file.".to_owned(),
        });
    }
    if fs::canonicalize(database_path).await.ok().as_deref() == Some(source.as_path()) {
        return Err(RuntimeError::InvalidRequest {
            detail: "The running database cannot be restored from itself.".to_owned(),
        });
    }
    if let Ok(directory) = fs::canonicalize(data_directory).await
        && source.starts_with(directory.join(RESTORE_DIRECTORY))
    {
        return Err(RuntimeError::InvalidRequest {
            detail: "That file is part of an earlier restore, not a backup to restore.".to_owned(),
        });
    }

    let info = peregrine_storage::inspect_backup(&source).await?;

    let directory = restore_directory(data_directory);
    fs::create_dir_all(&directory)
        .await
        .map_err(|error| io_failure(&directory, error))?;
    let pending = directory.join(PENDING);
    let previous = directory.join(PREVIOUS);
    remove_if_present(&pending).await?;
    remove_if_present(&previous).await?;

    // A consistent copy of the state being replaced, taken while the writer is running.
    progress(RestorePhase::Snapshotting);
    database.write_snapshot(&previous, &|_| {}).await?;

    progress(RestorePhase::Staging);
    let partial = directory.join(PARTIAL);
    remove_if_present(&partial).await?;
    if let Err(error) = fs::copy(&source, &partial).await {
        let _ = fs::remove_file(&partial).await;
        return Err(io_failure(&source, error));
    }
    fs::rename(&partial, &pending)
        .await
        .map_err(|error| io_failure(&directory, error))?;

    progress(RestorePhase::Staged);
    info!(
        source = %source.display(),
        schema_version = info.schema_version,
        "staged a database restore for the next launch"
    );
    Ok(info)
}

/// Applies a staged restore before the database is opened.
///
/// Returns whether this launch is the one that restored. Any failure after the live database
/// has been moved aside puts the previous database back, so a bad staged file is a failed
/// launch rather than a lost installation.
pub(crate) async fn apply_pending_restore(
    data_directory: &Path,
    database_path: &Path,
) -> Result<bool, RuntimeError> {
    let directory = restore_directory(data_directory);
    let pending = directory.join(PENDING);
    if !fs::try_exists(&pending).await.unwrap_or(false) {
        return Ok(false);
    }

    match apply(&directory, &pending, database_path).await {
        Ok(()) => {
            info!(
                database = %database_path.display(),
                "restored the database staged by the last launch"
            );
            Ok(true)
        }
        Err(error) => {
            if let Err(rollback) = roll_back_restore(data_directory, database_path).await {
                warn!(
                    %rollback,
                    "the previous database could not be put back after a failed restore"
                );
            }
            Err(error)
        }
    }
}

/// Puts the previous database back, keeping the restored file for diagnosis.
pub(crate) async fn roll_back_restore(
    data_directory: &Path,
    database_path: &Path,
) -> Result<(), RuntimeError> {
    let directory = restore_directory(data_directory);
    let previous = directory.join(PREVIOUS);
    let failed = directory.join(FAILED);

    if fs::try_exists(database_path).await.unwrap_or(false) {
        remove_if_present(&failed).await?;
        fs::rename(database_path, &failed)
            .await
            .map_err(|error| io_failure(database_path, error))?;
    }
    if fs::try_exists(&previous).await.unwrap_or(false) {
        remove_sidecars(database_path).await?;
        fs::rename(&previous, database_path)
            .await
            .map_err(|error| io_failure(database_path, error))?;
        info!(
            database = %database_path.display(),
            "put the previous database back"
        );
    }
    Ok(())
}

/// The moves a staged restore makes, with no rollback of its own.
async fn apply(directory: &Path, pending: &Path, database_path: &Path) -> Result<(), RuntimeError> {
    let previous = directory.join(PREVIOUS);
    if fs::try_exists(database_path).await.unwrap_or(false) {
        remove_if_present(&previous).await?;
        fs::rename(database_path, &previous)
            .await
            .map_err(|error| io_failure(database_path, error))?;
    }
    remove_sidecars(database_path).await?;
    fs::rename(pending, database_path)
        .await
        .map_err(|error| io_failure(database_path, error))
}

fn restore_directory(data_directory: &Path) -> PathBuf {
    data_directory.join(RESTORE_DIRECTORY)
}

async fn remove_sidecars(database_path: &Path) -> Result<(), RuntimeError> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = database_path.as_os_str().to_owned();
        sidecar.push(suffix);
        remove_if_present(Path::new(&sidecar)).await?;
    }
    Ok(())
}

async fn remove_if_present(path: &Path) -> Result<(), RuntimeError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_failure(path, error)),
    }
}

fn io_failure(path: &Path, source: std::io::Error) -> RuntimeError {
    StorageError::Backup {
        path: path.to_path_buf(),
        source: Box::new(source),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn write(path: &Path, bytes: &[u8]) {
        tokio::fs::write(path, bytes).await.unwrap();
    }

    async fn read(path: &Path) -> Vec<u8> {
        tokio::fs::read(path).await.unwrap()
    }

    /// The acceptance for the swap: the staged file becomes the database and the replaced
    /// one is kept, so a restore that opens badly has something to go back to.
    #[tokio::test]
    async fn a_staged_restore_replaces_the_database_and_keeps_the_previous_one() {
        let data = TempDir::new().unwrap();
        let database = data.path().join("peregrine.sqlite");
        write(&database, b"current").await;
        let directory = restore_directory(data.path());
        tokio::fs::create_dir_all(&directory).await.unwrap();
        write(&directory.join(PENDING), b"restored").await;

        assert!(apply_pending_restore(data.path(), &database).await.unwrap());

        assert_eq!(read(&database).await, b"restored");
        assert_eq!(read(&directory.join(PREVIOUS)).await, b"current");
        assert!(
            !directory.join(PENDING).exists(),
            "a staged file is consumed exactly once"
        );
    }

    #[tokio::test]
    async fn a_launch_with_nothing_staged_changes_nothing() {
        let data = TempDir::new().unwrap();
        let database = data.path().join("peregrine.sqlite");
        write(&database, b"current").await;

        assert!(!apply_pending_restore(data.path(), &database).await.unwrap());
        assert_eq!(read(&database).await, b"current");
    }

    /// A restore that cannot be opened leaves a failed file for diagnosis and the previous
    /// database in place, which is what makes the next launch work rather than start empty.
    #[tokio::test]
    async fn rolling_back_restores_the_previous_file_and_keeps_the_failed_one() {
        let data = TempDir::new().unwrap();
        let database = data.path().join("peregrine.sqlite");
        let directory = restore_directory(data.path());
        tokio::fs::create_dir_all(&directory).await.unwrap();
        write(&database, b"restored-but-unreadable").await;
        write(&directory.join(PREVIOUS), b"previous").await;

        roll_back_restore(data.path(), &database).await.unwrap();

        assert_eq!(read(&database).await, b"previous");
        assert_eq!(
            read(&directory.join(FAILED)).await,
            b"restored-but-unreadable"
        );
    }

    #[test]
    fn a_backup_name_is_utc_and_a_second_one_in_the_same_second_gets_a_suffix() {
        assert_eq!(
            backup_file_name(0),
            "peregrine-backup-1970-01-01-000000.sqlite"
        );
    }

    /// The acceptance end to end: a profile and its wishlist are written, backed up, removed,
    /// restored by a second launch, and read back. This is the half the file moves above cannot
    /// prove — that `Runtime::start` applies the stage before it opens the database.
    #[tokio::test]
    async fn a_backup_restores_profiles_and_wishlists_on_the_next_launch() {
        use std::sync::{Arc, Mutex};

        use crate::testing::{CountingSecretStore, RecordingSink};
        use crate::{EventSink, Observability, Runtime, RuntimeConfig};
        use peregrine_storage::{Database, StubTitle};

        async fn start(directory: &TempDir) -> Runtime {
            let sink = Arc::new(RecordingSink::default());
            let config = RuntimeConfig::new(
                directory.path(),
                directory.path().join("cache/images"),
                Arc::clone(&sink) as Arc<dyn EventSink>,
            )
            .with_secrets(Arc::new(CountingSecretStore::new()));
            let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
            drop(subscriber);
            Runtime::start(config, logging).await.unwrap()
        }

        let directory = TempDir::new().unwrap();
        let title = {
            let database = Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .unwrap();
            database
                .insert_stub_title(StubTitle {
                    kind: peregrine_domain::catalogue::TitleKind::Movie,
                    subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                    name: "The Matrix".to_owned(),
                    original_name: None,
                    sort_name: "the matrix".to_owned(),
                    at: 1_700_000_000_000,
                })
                .await
                .unwrap()
        };
        let backups = directory.path().join("backups");
        tokio::fs::create_dir_all(&backups).await.unwrap();

        let restore_phases = Arc::new(Mutex::new(Vec::new()));
        let profile = {
            let runtime = start(&directory).await;
            let profile = runtime.create_profile("Ada", "adult", None).await.unwrap();
            runtime.add_to_watchlist(title).await.unwrap();
            let backup = runtime.create_backup(&backups).await.unwrap();
            runtime.remove_from_watchlist(title).await.unwrap();
            let sink = Arc::clone(&restore_phases);
            runtime
                .restore_backup_with_progress(&backup.path, &move |phase| {
                    sink.lock()
                        .expect("the phase log is not poisoned")
                        .push(phase);
                })
                .await
                .unwrap();
            runtime.shutdown().await;
            profile
        };

        assert_eq!(
            *restore_phases
                .lock()
                .expect("the phase log is not poisoned"),
            vec![
                RestorePhase::Snapshotting,
                RestorePhase::Staging,
                RestorePhase::Staged,
            ],
            "the bar sees every restore phase before the restart"
        );

        let runtime = start(&directory).await;
        let profiles = runtime.profiles().await.unwrap();
        assert!(
            profiles
                .iter()
                .any(|row| row.id == profile.id && row.name == "Ada"),
            "the profile is back: {profiles:?}"
        );
        assert_eq!(
            runtime.watchlist().await.unwrap().len(),
            1,
            "the wishlist is back"
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn free_destination_never_reuses_a_name() {
        let directory = TempDir::new().unwrap();
        let first = free_destination(directory.path(), "peregrine-backup-x.sqlite")
            .await
            .unwrap();
        write(&first, b"one").await;
        let second = free_destination(directory.path(), "peregrine-backup-x.sqlite")
            .await
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(second, directory.path().join("peregrine-backup-x-2.sqlite"));
    }
}
