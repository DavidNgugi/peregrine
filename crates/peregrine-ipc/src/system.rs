//! The `system` namespace of the command surface (docs/ARCHITECTURE.md §7).

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use specta::Type;

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
}
