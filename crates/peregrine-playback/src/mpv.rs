//! The mpv adapter: a hardened external player for containers the webview cannot demux.
//!
//! **This is the one place a downloaded file is handed to another program**, so SECURITY.md T5 and
//! ARCHITECTURE §510 are the shape of it: no shell, an argument array, the file after `--` so a name
//! beginning with a hyphen cannot become a flag, the fixed flag set that disables config files,
//! scripts, ytdl, and automatic subtitle loading, and an environment reduced to the set a window
//! needs. The binary is resolved by name rather than by a user-supplied string.
//!
//! mpv opens its own window; the process is this session and is replaced when another file is
//! played. A player the user closes is reaped on the next press, and the app stops it on shutdown.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use peregrine_domain::{PlayerBackend, PlayerError, SafePath};

/// The flags every spawn carries (SECURITY.md T5, ARCHITECTURE §510).
///
/// `--` is added separately, immediately before the file, because it is the end of option parsing
/// rather than a setting.
const HARDENED: &[&str] = &[
    "--no-config",
    "--load-scripts=no",
    "--no-ytdl",
    "--sub-auto=no",
    "--script-opts=",
    "--no-terminal",
];

/// The variables a window needs, and nothing else, for the same reason the flags are fixed.
const ENVIRONMENT: &[&str] = &[
    "HOME",
    "PATH",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
];

/// The binary name mpv installs under on this platform.
const MPV_BINARY: &str = if cfg!(windows) { "mpv.exe" } else { "mpv" };

/// The places a desktop install puts mpv when the app was launched from the launcher rather than a
/// shell, where PATH is short.
#[cfg(target_os = "macos")]
const COMMON: &[&str] = &[
    "/opt/homebrew/bin/mpv",
    "/usr/local/bin/mpv",
    "/Applications/mpv.app/Contents/MacOS/mpv",
];
#[cfg(target_os = "windows")]
const COMMON: &[&str] = &[
    r"C:\Program Files\mpv\mpv.exe",
    r"C:\Program Files (x86)\mpv\mpv.exe",
];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const COMMON: &[&str] = &["/usr/bin/mpv", "/usr/local/bin/mpv"];

/// The shipping [`PlayerBackend`]: mpv, in a window of its own.
pub struct MpvPlayer {
    child: Mutex<Option<Child>>,
    /// Overrides the resolved binary, which is what a test uses to exercise the spawn path without
    /// a player on the machine. `None` resolves at each press, so installing mpv is picked up.
    binary: Option<PathBuf>,
}

impl MpvPlayer {
    /// The player, resolving mpv when a file is played.
    #[must_use]
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
            binary: None,
        }
    }

    /// The same player, always spawning `binary`. A test seam rather than a setting.
    #[must_use]
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            child: Mutex::new(None),
            binary: Some(binary.into()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for MpvPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerBackend for MpvPlayer {
    fn play(&self, file: &SafePath) -> Result<(), PlayerError> {
        // Replacing the player is the same act as stopping the old one, and doing it first means a
        // spawn that fails still leaves no zombie behind.
        self.stop();
        let binary = match &self.binary {
            Some(path) => path.clone(),
            None => resolve_mpv().ok_or(PlayerError::NotInstalled)?,
        };
        let mut command = command_for(&binary, file.as_path());
        let child = command.spawn().map_err(|_| PlayerError::Spawn)?;
        *self.lock() = Some(child);
        Ok(())
    }

    fn stop(&self) {
        let child = self.lock().take();
        if let Some(mut child) = child {
            // A player the user already closed has exited; a failed kill is not a refusal to report.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        let child = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The command for one file, with the fixed flags and the file behind `--`.
fn command_for(binary: &Path, file: &Path) -> Command {
    let mut command = Command::new(binary);
    command.args(HARDENED);
    // The end of option parsing. A canonical path begins with a separator on every platform this
    // build runs on, so this is belt and braces; it is also the rule SECURITY.md T5 states.
    command.arg("--");
    command.arg(file);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command.env_clear();
    for key in ENVIRONMENT {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}

/// mpv on PATH, or in a place a desktop install puts it. A missing binary is the caller's to report.
fn resolve_mpv() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(MPV_BINARY);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    COMMON
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use peregrine_domain::PathGuard;

    fn file(directory: &tempfile::TempDir) -> SafePath {
        let path = directory.path().join("film.mkv");
        std::fs::write(&path, b"fixture").expect("the fixture is written");
        PathGuard::new(directory.path())
            .expect("the fixture root is absolute")
            .resolve(&path)
            .expect("the file is inside its root")
    }

    #[test]
    fn the_flags_disable_config_scripts_ytdl_and_subtitle_autoload() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let command = command_for(Path::new("/usr/bin/true"), file(&directory).as_path());
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        for flag in [
            "--no-config",
            "--load-scripts=no",
            "--no-ytdl",
            "--sub-auto=no",
            "--script-opts=",
            "--no-terminal",
        ] {
            assert!(
                args.iter().any(|arg| arg.to_string_lossy() == flag),
                "{flag}"
            );
        }
        // The file comes after the end-of-options marker, so a name beginning with a hyphen is a
        // path rather than a flag.
        let terminator = args
            .iter()
            .position(|arg| arg.to_string_lossy() == "--")
            .expect("the terminator is there");
        assert_eq!(args.len(), terminator + 2, "exactly the file follows --");
    }

    #[test]
    fn a_file_is_spawned_and_the_previous_child_is_replaced() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let safe = file(&directory);
        // `/usr/bin/true` ignores the flags and exits, which is enough to prove the spawn path and
        // the replacement without a player installed.
        let player = MpvPlayer::with_binary("/usr/bin/true");
        player.play(&safe).expect("the binary spawns");
        player
            .play(&safe)
            .expect("a second press replaces the first");
        player.stop();
    }

    #[test]
    fn a_missing_binary_is_reported_rather_than_silently_doing_nothing() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let player = MpvPlayer::with_binary("/nonexistent/peregrine-mpv");
        let error = player.play(&file(&directory)).expect_err("no such binary");
        assert_eq!(error, PlayerError::Spawn);
    }
}
