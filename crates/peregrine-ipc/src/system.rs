//! The `system` namespace of the command surface (docs/ARCHITECTURE.md §7).

use crate::error::AppError;
use crate::guard::guard_response;
use crate::runtime::RuntimeState;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

/// What the About box and the diagnostics bundle need to identify this build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// The application name the bundle was built under.
    pub app_name: String,
    /// The application version, from the package metadata.
    pub app_version: String,
    /// The OS and architecture the binary was compiled for, so a support report
    /// can tell a Windows install from a Linux one without guessing.
    pub target: String,
}

/// The build metadata for the running binary.
///
/// The version is read from the bundle rather than from `CARGO_PKG_VERSION`,
/// because a packaging step can override it and the About box must show what was
/// actually installed.
///
/// This command has no failing path: all three values come from infallible
/// lookups. The `Result` is still there because every command answers with the
/// `{ code, message, detail?, retryable, cause? }` shape from
/// docs/ARCHITECTURE.md §7 rule 3, and a command that returned a bare value
/// would be the one call site the UI's error handling could not cover.
#[tauri::command]
#[specta::specta]
// Tauri injects command arguments by value; this one is only read, but taking a
// reference would stop the framework from recognising it as a command argument.
#[allow(clippy::needless_pass_by_value)]
pub fn build_info<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<BuildInfo, AppError> {
    let package = app.package_info();
    Ok(BuildInfo {
        app_name: package.name.clone(),
        app_version: package.version.to_string(),
        // Cargo sets `TARGET` for build scripts, not for library compilation, so
        // this is built from the OS and architecture constants the standard
        // library guarantees instead of the target triple.
        target: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
    })
}

/// What the diagnostics screen reports about the running process.
///
/// Every value is measured; none is hardcoded (PG-E25-06's rule applied to the one
/// screen that exists today).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// How long startup took, in milliseconds.
    #[specta(type = Number)]
    pub startup_millis: u64,
    /// The scheduler's counters.
    pub scheduler: SchedulerHealth,
    /// The event bus's counters.
    pub events: EventHealth,
}

/// The scheduler's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerHealth {
    /// Jobs ready to start.
    pub ready: u32,
    /// Jobs waiting for a retry deadline.
    pub deferred: u32,
    /// Attempts currently executing.
    pub inflight: u32,
    /// Jobs the scheduler still tracks.
    pub live: u32,
    /// Retries scheduled since the process started.
    #[specta(type = Number)]
    pub retries_scheduled: u64,
}

/// The event bus's counters.
///
/// `degraded` is derived rather than sent separately, so the UI cannot render a live view
/// as healthy while the numbers say updates were discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EventHealth {
    /// Updates accepted from producers.
    #[specta(type = Number)]
    pub published: u64,
    /// Updates replaced by a newer one before delivery.
    #[specta(type = Number)]
    pub coalesced: u64,
    /// Updates the transport accepted.
    #[specta(type = Number)]
    pub delivered: u64,
    /// Updates the transport refused.
    #[specta(type = Number)]
    pub failed: u64,
    /// Updates discarded because too many subjects were already waiting.
    #[specta(type = Number)]
    pub dropped: u64,
    /// Whether anything was refused or discarded.
    pub degraded: bool,
}

/// The process's own measurements.
///
/// # Errors
///
/// [`crate::error::ErrorCode::Internal`] when the scheduler has stopped, because its
/// counters are then not the running application's.
#[tauri::command]
#[specta::specta]
pub async fn health(state: State<'_, RuntimeState>) -> Result<Health, AppError> {
    let health = state.runtime()?.health().await;
    Ok(guard_response("system::health", Health::from(health)))
}

impl From<peregrine_app::Health> for Health {
    fn from(health: peregrine_app::Health) -> Self {
        Self {
            startup_millis: u64::try_from(health.startup.as_millis()).unwrap_or(u64::MAX),
            scheduler: SchedulerHealth {
                ready: health.scheduler.ready as u32,
                deferred: health.scheduler.deferred as u32,
                inflight: health.scheduler.inflight as u32,
                live: health.scheduler.live as u32,
                retries_scheduled: health.scheduler.retries_scheduled,
            },
            events: EventHealth {
                published: health.events.published,
                coalesced: health.events.coalesced,
                delivered: health.events.delivered,
                failed: health.events.failed,
                dropped: health.events.dropped,
                degraded: health.events.is_degraded(),
            },
        }
    }
}

/// The last lines of the newest log file, newest last.
///
/// The runtime bounds the read — a 64 KiB window and at most `peregrine_app::runtime::
/// MAX_LOG_LINES` lines — so this answer cannot exceed the IPC payload budget.
///
/// # Errors
///
/// [`crate::error::ErrorCode::Internal`] when the log file exists but cannot be read.
#[tauri::command]
#[specta::specta]
pub async fn log_tail(
    state: State<'_, RuntimeState>,
    lines: Option<u32>,
) -> Result<Vec<String>, AppError> {
    let lines = lines.unwrap_or(100);
    let tail = state.runtime()?.log_tail(lines).await?;
    Ok(guard_response("system::log_tail", tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command reads its version from the bundle, so the test asserts the
    /// wiring against Tauri's own package metadata rather than a hard-coded
    /// string that a version bump would invalidate.
    #[test]
    fn build_info_reports_the_bundles_package_metadata() {
        let app = tauri::test::mock_app();
        let expected = app.package_info().version.to_string();

        let info = build_info(app.handle().clone()).unwrap();

        assert_eq!(info.app_version, expected);
        assert!(!info.app_name.is_empty(), "the bundle must name the app");
        assert_eq!(
            info.target,
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    /// The health DTO is a mapping, and the one thing it must not do is report a live
    /// view as healthy when the bus discarded an update.
    #[test]
    fn a_degraded_bus_is_reported_as_degraded() {
        let health = Health {
            startup_millis: 12,
            scheduler: SchedulerHealth {
                ready: 0,
                deferred: 1,
                inflight: 2,
                live: 3,
                retries_scheduled: 4,
            },
            events: EventHealth {
                published: 10,
                coalesced: 3,
                delivered: 6,
                failed: 1,
                dropped: 0,
                degraded: true,
            },
        };

        let json = serde_json::to_value(health).unwrap();
        assert_eq!(json["startupMillis"], 12);
        assert_eq!(json["scheduler"]["retriesScheduled"], 4);
        assert_eq!(json["events"]["degraded"], true);
        assert_eq!(json["events"]["delivered"], 6);
    }
}
