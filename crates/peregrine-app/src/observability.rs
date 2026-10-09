//! The tracing subscriber: structured JSON into daily-rotated files, with a
//! level filter that can be replaced on a running process.
//!
//! The subscriber is built once at startup and installed with
//! [`Observability::install`]. Every event passes through the redaction filter
//! from `peregrine-secrets`, so the files never contain a registered secret, and
//! a settings change reaches a running process through [`Observability::set_level`]
//! instead of a restart.

use std::path::PathBuf;

use peregrine_secrets::RedactingFormat;
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;
use tracing_subscriber::filter::ParseError;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::reload;

/// The name shared by every rotated file.
const FILE_PREFIX: &str = "peregrine";
/// The suffix that marks a file as one JSON object per line.
const FILE_SUFFIX: &str = "jsonl";

/// Where the logs are written and which filter is in force at startup.
#[derive(Debug)]
pub struct LogConfig {
    /// The directory the daily files are written to.
    ///
    /// Taken as a parameter rather than resolved here: the per-OS directory
    /// helper does not exist yet, and a hard-coded location could not be tested
    /// without writing into the developer's home directory (AGENTS R14).
    pub directory: PathBuf,
    /// `EnvFilter` directives, for example `info,peregrine_app=debug`.
    ///
    /// The caller resolves these from the settings registry (`PG-E20-10`) or,
    /// for the homelab daemon, the environment overlay (`PG-E36-01`); this
    /// module applies them and replaces them on [`Observability::set_level`].
    pub directives: String,
}

/// The handles a running subscriber must keep alive.
///
/// Dropping the value flushes the queued lines and stops the writer thread, so
/// the application holds it for the process lifetime.
pub struct Observability {
    reload: reload::Handle<EnvFilter, Registry>,
    // Never read: held for its `Drop`, which flushes the queued lines and stops
    // the writer thread. Losing it would silently drop lines on exit.
    _guard: WorkerGuard,
}

impl Observability {
    /// Builds the subscriber without installing it.
    ///
    /// The subscriber is returned alongside the handles so that a test can scope
    /// it to one thread with [`tracing::subscriber::set_default`]; the
    /// application calls [`Observability::install`] instead.
    pub fn build(
        config: &LogConfig,
    ) -> Result<(impl Subscriber + Send + Sync + 'static, Self), ObservabilityError> {
        let filter = EnvFilter::try_new(&config.directives).map_err(|source| {
            ObservabilityError::Filter {
                directives: config.directives.clone(),
                source,
            }
        })?;
        let (filter, reload) = reload::Layer::new(filter);

        // The builder rather than `rolling::daily`: the latter panics when the
        // directory cannot be created, and a startup path reports instead.
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(FILE_PREFIX)
            .filename_suffix(FILE_SUFFIX)
            .build(&config.directory)
            .map_err(|source| ObservabilityError::Directory {
                path: config.directory.clone(),
                source,
            })?;
        let (writer, guard) = tracing_appender::non_blocking(appender);

        let subscriber = Registry::default().with(filter).with(
            fmt::layer()
                .event_format(RedactingFormat::new(fmt::format::Format::default().json()))
                .with_writer(writer),
        );

        Ok((
            subscriber,
            Self {
                reload,
                _guard: guard,
            },
        ))
    }

    /// Installs the subscriber as the process-wide default.
    pub fn install(config: &LogConfig) -> Result<Self, ObservabilityError> {
        let (subscriber, observability) = Self::build(config)?;
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|source| ObservabilityError::AlreadyInstalled { source })?;
        Ok(observability)
    }

    /// Applies `directives` to the running subscriber.
    ///
    /// This is the whole point of the reload handle: the change takes effect for
    /// the next event, with no restart and no second subscriber.
    pub fn set_level(&self, directives: &str) -> Result<(), ObservabilityError> {
        let filter =
            EnvFilter::try_new(directives).map_err(|source| ObservabilityError::Filter {
                directives: directives.to_owned(),
                source,
            })?;
        self.reload
            .reload(filter)
            .map_err(|source| ObservabilityError::Reconfigure { source })
    }
}

/// Why the subscriber could not be built, installed, or reconfigured.
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    /// The directives are not a valid filter expression.
    #[error("`{directives}` is not a valid log filter: {source}")]
    Filter {
        /// The rejected directives.
        directives: String,
        /// The parser's error.
        source: ParseError,
    },

    /// The log directory could not be opened.
    #[error("cannot open the log directory {}: {source}", path.display())]
    Directory {
        /// The directory that failed.
        path: PathBuf,
        /// The appender's error.
        source: tracing_appender::rolling::InitError,
    },

    /// The process already has a global subscriber, so this one was not used.
    #[error("a global log subscriber is already installed: {source}")]
    AlreadyInstalled {
        /// The installer's error.
        source: tracing::subscriber::SetGlobalDefaultError,
    },

    /// The subscriber that owns the filter is gone, or its lock is poisoned.
    #[error("the running log subscriber could not be reconfigured: {source}")]
    Reconfigure {
        /// The reload failure.
        source: reload::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use peregrine_secrets::Secret;

    /// Every byte the pipeline wrote, across every file under `directory`.
    fn read_all(directory: &Path) -> String {
        let mut text = String::new();
        let mut pending = vec![directory.to_path_buf()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(&next).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    // Lossy rather than `read_to_string`: a log file with a byte
                    // that is not UTF-8 must still be scanned.
                    text.push_str(&String::from_utf8_lossy(&fs::read(&path).unwrap()));
                }
            }
        }
        text
    }

    fn log_config(directory: &Path, directives: &str) -> LogConfig {
        LogConfig {
            directory: directory.to_path_buf(),
            directives: directives.to_owned(),
        }
    }

    #[test]
    fn a_level_change_applies_to_a_running_subscriber() {
        let directory = tempfile::tempdir().unwrap();
        let config = log_config(directory.path(), "info,peregrine_app=info");
        let (subscriber, observability) = Observability::build(&config).unwrap();

        {
            let _default = tracing::subscriber::set_default(subscriber);
            tracing::debug!(target: "peregrine_app", "debug-before-the-level-change");
            tracing::info!(target: "peregrine_app", "info-before-the-level-change");
            tracing::info!(target: "peregrine_playback", "info-from-another-module");

            observability
                .set_level("info,peregrine_app=debug")
                .expect("the new directives are valid");

            tracing::debug!(target: "peregrine_app", "debug-after-the-level-change");
            tracing::debug!(target: "peregrine_playback", "debug-from-another-module");
        }

        // Dropping the guard stops the writer thread after it has flushed every
        // queued line, so the files are complete before they are read.
        drop(observability);

        let logs = read_all(directory.path());
        assert!(logs.contains("info-before-the-level-change"), "{logs}");
        assert!(!logs.contains("debug-before-the-level-change"), "{logs}");
        assert!(logs.contains("debug-after-the-level-change"), "{logs}");
        // The filter is per module: the bare `info` default still applies to
        // everything the directive does not name.
        assert!(logs.contains("info-from-another-module"), "{logs}");
        assert!(!logs.contains("debug-from-another-module"), "{logs}");
    }

    #[test]
    fn a_registered_secret_never_reaches_a_rotated_file() {
        const SECRET: &str = "app-token-4f9c2b1a8e7d6c5b";
        const UNREGISTERED: &str = "plugin-token-never-registered";

        let secret = Secret::new(SECRET);
        let directory = tempfile::tempdir().unwrap();
        let config = log_config(directory.path(), "info");
        let (subscriber, observability) = Observability::build(&config).unwrap();

        {
            let _default = tracing::subscriber::set_default(subscriber);
            tracing::info!(provider = "tmdb", token = %secret.expose_secret(), "authenticating");
            tracing::info!(provider = "tvdb", token = UNREGISTERED, "authenticating");
        }
        drop(observability);

        let logs = read_all(directory.path());
        assert!(!logs.contains(SECRET), "{logs}");
        assert!(!logs.contains("4f9c2b1a"), "{logs}");
        assert!(logs.contains("[REDACTED]"), "{logs}");
        // The negative control: the same scan finds a value that was never
        // registered, so a green run cannot be a scan of nothing.
        assert!(logs.contains(UNREGISTERED), "{logs}");
    }

    #[test]
    fn installing_twice_reports_the_existing_subscriber() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();

        let installed = Observability::install(&log_config(first.path(), "info"))
            .expect("the first global install must succeed");
        // `let ... else` rather than `expect_err`: the success value is a
        // subscriber stack, which has no `Debug` impl to print.
        let Err(error) = Observability::install(&log_config(second.path(), "info")) else {
            panic!("a second global subscriber cannot be installed");
        };

        assert!(
            matches!(error, ObservabilityError::AlreadyInstalled { .. }),
            "{error:?}"
        );

        tracing::info!("written-through-the-installed-subscriber");
        drop(installed);

        let logs = read_all(first.path());
        assert!(
            logs.contains("written-through-the-installed-subscriber"),
            "{logs}"
        );
    }

    #[test]
    fn an_invalid_filter_is_reported_with_its_directives() {
        let directory = tempfile::tempdir().unwrap();
        let error = Observability::build(&log_config(directory.path(), "peregrine_app=nonsense"))
            .err()
            .expect("`nonsense` is not a level");

        assert!(
            matches!(error, ObservabilityError::Filter { .. }),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("peregrine_app=nonsense"),
            "{error}"
        );
    }

    #[test]
    fn an_uncreatable_log_directory_is_reported_not_panicked() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("not-a-directory");
        fs::write(&file, b"a file, not a log directory").unwrap();

        let error = Observability::build(&log_config(&file, "info"))
            .err()
            .expect("a file cannot hold the log files");

        assert!(
            matches!(error, ObservabilityError::Directory { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("not-a-directory"), "{error}");
    }
}
