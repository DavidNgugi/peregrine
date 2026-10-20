//! The mpv adapter: a hardened external player for containers the webview cannot demux.
//!
//! **This is the one place a downloaded file is handed to another program**, so SECURITY.md T5 and
//! ARCHITECTURE §510 are the shape of it: no shell, an argument array, the file after `--` so a name
//! beginning with a hyphen cannot become a flag, the fixed flag set that disables config files,
//! scripts, ytdl, and automatic subtitle loading, and an environment reduced to the set a window
//! needs. The binary is resolved by name rather than by a user-supplied string.
//!
//! **The position comes back over mpv's IPC socket.** A child started with `--input-ipc-server`
//! gets one JSON event per line; this module observes `time-pos` and `duration` and reports the
//! former through [`PositionSink`] at the same five-second media cadence the in-page player uses.
//! The socket is a file under the temp directory (a named pipe on Windows), one per play, and it
//! dies with the child.
//!
//! mpv opens its own window; the process is this session and is replaced when another file is
//! played. A player the user closes is reaped on the next press, and the app stops it on shutdown.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use peregrine_domain::{PlayerBackend, PlayerError, PlayerInput, PositionSink, SafePath};

/// How much media time must pass between two position reports.
const REPORT_EVERY_SECONDS: f64 = 5.0;
/// How long to wait for mpv to create its IPC socket before giving up.
const CONNECT_ATTEMPTS: u32 = 40;
const CONNECT_RETRY: Duration = Duration::from_millis(50);

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
    // The app owns the media keys and the Now Playing centre (PG-E16-21); a child that also
    // registered for them paused when another app sent a play/pause, which looked like the player
    // freezing. The child is a video surface, not a media application.
    "--input-media-keys=no",
    "--media-controls=no",
    // The app's own watch progress is the resume rule; mpv's watch_later must not override it.
    "--no-resume-playback",
    // A fresh child starts audible whatever a previous session did with the mute key.
    "--volume=100",
    "--mute=no",
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

/// The child this player started, and the socket a reader uses to hear its position.
struct Session {
    child: Child,
    /// A socket file on unix, removed on stop; a named pipe on Windows, which needs no cleanup.
    #[cfg_attr(windows, allow(dead_code))]
    socket: PathBuf,
    stop: Arc<AtomicBool>,
}

/// The shipping [`PlayerBackend`]: mpv, in a window of its own.
pub struct MpvPlayer {
    session: Mutex<Option<Session>>,
    /// Overrides the resolved binary, which is what a test uses to exercise the spawn path without
    /// a player on the machine. `None` resolves at each press, so installing mpv is picked up.
    binary: Option<PathBuf>,
    next_socket: AtomicU64,
}

impl MpvPlayer {
    /// The player, resolving mpv when a file is played.
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            binary: None,
            next_socket: AtomicU64::new(1),
        }
    }

    /// The same player, always spawning `binary`: a bundled sidecar, or a test fixture.
    #[must_use]
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            session: Mutex::new(None),
            binary: Some(binary.into()),
            next_socket: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.session
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
    fn play(
        &self,
        input: PlayerInput<'_>,
        start_ms: Option<i64>,
        subtitle: Option<&SafePath>,
        positions: Arc<dyn PositionSink>,
    ) -> Result<(), PlayerError> {
        // Replacing the player is the same act as stopping the old one, and doing it first means a
        // spawn that fails still leaves no zombie behind.
        self.stop();
        let binary = match &self.binary {
            Some(path) => path.clone(),
            None => resolve_mpv().ok_or(PlayerError::NotInstalled)?,
        };
        let serial = self.next_socket.fetch_add(1, Ordering::Relaxed);
        let socket = control_path(serial);
        let stop = Arc::new(AtomicBool::new(false));
        let mut command = command_for(
            &binary,
            input,
            subtitle.map(SafePath::as_path),
            &socket,
            start_ms,
        );
        let child = command.spawn().map_err(|_| PlayerError::Spawn)?;
        *self.lock() = Some(Session {
            child,
            socket: socket.clone(),
            stop: Arc::clone(&stop),
        });
        if std::thread::Builder::new()
            .name("peregrine-mpv-ipc".to_owned())
            .spawn(move || read_positions(&socket, &stop, positions.as_ref()))
            .is_err()
        {
            self.stop();
            return Err(PlayerError::Spawn);
        }
        Ok(())
    }

    fn stop(&self) {
        let session = self.lock().take();
        if let Some(session) = session {
            session.stop.store(true, Ordering::Relaxed);
            let mut child = session.child;
            // A player the user already closed has exited; a failed kill is not a refusal to report.
            let _ = child.kill();
            let _ = child.wait();
            #[cfg(unix)]
            let _ = std::fs::remove_file(&session.socket);
        }
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(session) = session {
            session.stop.store(true, Ordering::Relaxed);
            let mut child = session.child;
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The command for one file, with the fixed flags and the file behind `--`.
///
/// A selected sidecar is passed as `--sub-file=`, and it is the same guarded `SafePath` the media
/// went through: `--sub-auto=no` is what stops mpv loading a neighbour nobody chose.
fn command_for(
    binary: &Path,
    input: PlayerInput<'_>,
    subtitle: Option<&Path>,
    socket: &Path,
    start_ms: Option<i64>,
) -> Command {
    let mut command = Command::new(binary);
    command.args(HARDENED);
    command.arg(format!("--input-ipc-server={}", socket.display()));
    if let Some(milliseconds) = start_ms.filter(|value| *value > 0) {
        command.arg(format!("--start={:.3}", milliseconds as f64 / 1000.0));
    }
    if matches!(input, PlayerInput::Stream(_)) {
        // **A torrent in flight may not have its last pieces, and libavformat's probe seeks to the
        // Matroska Cues at the end for them.** An unseekable stream is read forward instead, which
        // is the only read a stalled tail can answer. `--start` still lands on the requested frame:
        // the reader discards forward to it rather than seeking. Seeking during playback is the
        // cost, and it is the right trade for a stream that is still arriving.
        command.arg("--stream-lavf-o=seekable=0");
    }
    // A sidecar belongs to a library file. A stream has no `subtitle_track` row to hang one from,
    // and the caller passes none; ignoring one here rather than failing keeps a file-shaped option
    // from becoming a reason a film will not start.
    if let (PlayerInput::File(_), Some(subtitle)) = (input, subtitle) {
        // An OsString rather than a formatted string: a path need not be UTF-8.
        let mut flag = std::ffi::OsString::from("--sub-file=");
        flag.push(subtitle.as_os_str());
        command.arg(flag);
    }
    // The end of option parsing. A canonical path begins with a separator on every platform this
    // build runs on, so this is belt and braces; it is also the rule SECURITY.md T5 states. It
    // applies to the URL too: mpv's own option parser must see it as the item, not as a flag.
    command.arg("--");
    match input {
        PlayerInput::File(file) => {
            command.arg(file.as_path());
        }
        PlayerInput::Stream(url) => {
            command.arg(url.as_str());
        }
    }
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

/// One socket per play, so a stale reader cannot hear the next session.
#[cfg(windows)]
fn control_path(serial: u64) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\peregrine-mpv-{}-{serial}",
        std::process::id()
    ))
}
#[cfg(not(windows))]
fn control_path(serial: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "peregrine-mpv-{}-{serial}.sock",
        std::process::id()
    ))
}

/// The read/write pair a control socket needs, without a platform-specific type in the signature.
trait Control: std::io::Read + std::io::Write + Send {}
impl<T: std::io::Read + std::io::Write + Send> Control for T {}

fn connect(socket: &Path, stop: &AtomicBool) -> Option<Box<dyn Control>> {
    for _ in 0..CONNECT_ATTEMPTS {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(control) = connect_once(socket) {
            return Some(control);
        }
        std::thread::sleep(CONNECT_RETRY);
    }
    None
}

#[cfg(unix)]
fn connect_once(socket: &Path) -> Option<Box<dyn Control>> {
    std::os::unix::net::UnixStream::connect(socket)
        .ok()
        .map(|stream| Box::new(stream) as Box<dyn Control>)
}
#[cfg(windows)]
fn connect_once(socket: &Path) -> Option<Box<dyn Control>> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(socket)
        .ok()
        .map(|file| Box::new(file) as Box<dyn Control>)
}

/// Reads mpv's position events until the socket closes or the session is stopped.
fn read_positions(socket: &Path, stop: &AtomicBool, sink: &dyn PositionSink) {
    let Some(mut control) = connect(socket, stop) else {
        return;
    };
    let request = concat!(
        "{\"command\":[\"observe_property\",1,\"time-pos\"]}\n",
        "{\"command\":[\"observe_property\",2,\"duration\"]}\n"
    );
    if control.write_all(request.as_bytes()).is_err() || control.flush().is_err() {
        return;
    }
    let mut reader = BufReader::new(control);
    let mut position: Option<f64> = None;
    let mut duration: Option<f64> = None;
    let mut last_reported = f64::NEG_INFINITY;
    let mut line = String::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        match event.get("event").and_then(serde_json::Value::as_str) {
            Some("property-change") => {
                let data = event.get("data").and_then(serde_json::Value::as_f64);
                match event.get("id").and_then(serde_json::Value::as_i64) {
                    Some(1) => {
                        if let Some(value) = data {
                            position = Some(value);
                            if (value - last_reported).abs() >= REPORT_EVERY_SECONDS {
                                last_reported = value;
                                sink.position(millis(value), duration.map(millis));
                            }
                        }
                    }
                    Some(2) => duration = data,
                    _ => {}
                }
            }
            Some("end-file") => break,
            _ => {}
        }
    }
    // The last position, so leaving the player is recorded even if no tick landed since the last one.
    if let Some(value) = position {
        sink.position(millis(value), duration.map(millis));
    }
}

fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round().clamp(0.0, i64::MAX as f64) as i64
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
    use peregrine_domain::{LoopbackUrl, PathGuard};

    struct Noop;

    impl PositionSink for Noop {
        fn position(&self, _position_ms: i64, _duration_ms: Option<i64>) {}
    }

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
        let media = file(&directory);
        let command = command_for(
            Path::new("/usr/bin/true"),
            PlayerInput::File(&media),
            None,
            Path::new("/tmp/peregrine-mpv-test.sock"),
            None,
        );
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        for flag in [
            "--no-config",
            "--load-scripts=no",
            "--no-ytdl",
            "--sub-auto=no",
            "--script-opts=",
            "--no-terminal",
            "--input-media-keys=no",
            "--media-controls=no",
            "--no-resume-playback",
            "--volume=100",
            "--mute=no",
        ] {
            assert!(
                args.iter().any(|arg| arg.to_string_lossy() == flag),
                "{flag}"
            );
        }
        assert!(
            args.iter()
                .any(|arg| arg.to_string_lossy().starts_with("--input-ipc-server=")),
            "the position listener needs a socket"
        );
        // The file comes after the end-of-options marker, so a name beginning with a hyphen is a
        // path rather than a flag.
        let terminator = args
            .iter()
            .position(|arg| arg.to_string_lossy() == "--")
            .expect("the terminator is there");
        assert_eq!(args.len(), terminator + 2, "exactly the file follows --");
        assert!(
            !args
                .iter()
                .any(|arg| arg.to_string_lossy() == "--stream-lavf-o=seekable=0"),
            "a file on disk stays seekable"
        );
    }

    #[test]
    fn a_selected_sidecar_is_passed_while_autoload_stays_off() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let media = file(&directory);
        let sidecar = directory.path().join("film.en.srt");
        let command = command_for(
            Path::new("/usr/bin/true"),
            PlayerInput::File(&media),
            Some(&sidecar),
            Path::new("/tmp/peregrine-mpv-test.sock"),
            None,
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter()
                .any(|arg| arg == &format!("--sub-file={}", sidecar.display())),
            "the chosen sidecar is the one mpv is told to show"
        );
        assert!(args.iter().any(|arg| arg == "--sub-auto=no"));
        let terminator = args.iter().position(|arg| arg == "--").expect("terminator");
        assert!(
            args.iter()
                .position(|arg| arg.starts_with("--sub-file="))
                .is_some_and(|at| at < terminator),
            "the flag is an option, not a second file after --"
        );
    }

    #[test]
    fn a_resume_position_is_passed_as_whole_and_millisecond_seconds() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let media = file(&directory);
        let command = command_for(
            Path::new("/usr/bin/true"),
            PlayerInput::File(&media),
            None,
            Path::new("/tmp/peregrine-mpv-test.sock"),
            Some(90_500),
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|arg| arg == "--start=90.500"));
    }

    #[test]
    fn a_zero_start_is_not_passed_at_all() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let media = file(&directory);
        let command = command_for(
            Path::new("/usr/bin/true"),
            PlayerInput::File(&media),
            None,
            Path::new("/tmp/peregrine-mpv-test.sock"),
            Some(0),
        );
        assert!(
            !command
                .get_args()
                .any(|arg| arg.to_string_lossy().starts_with("--start=")),
            "the beginning is the default, not a flag"
        );
    }

    #[test]
    fn a_loopback_stream_follows_the_terminator_and_takes_no_sidecar() {
        let url = LoopbackUrl::parse("http://127.0.0.1:43123/media/token").expect("a loopback URL");
        let command = command_for(
            Path::new("/usr/bin/true"),
            PlayerInput::Stream(&url),
            Some(Path::new("/tmp/film.en.srt")),
            Path::new("/tmp/peregrine-mpv-test.sock"),
            Some(15_000),
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let terminator = args.iter().position(|arg| arg == "--").expect("terminator");
        assert_eq!(args.len(), terminator + 2, "exactly the stream follows --");
        assert_eq!(args.last().map(String::as_str), Some(url.as_str()));
        assert!(
            !args.iter().any(|arg| arg.starts_with("--sub-file=")),
            "a stream has no sidecar row"
        );
        assert!(args.iter().any(|arg| arg == "--start=15.000"));
        assert!(
            args.iter().any(|arg| arg == "--stream-lavf-o=seekable=0"),
            "a torrent stream must not make libavformat seek to a missing tail"
        );
    }

    #[test]
    fn a_stream_is_spawned_through_the_same_hardened_path() {
        let url = LoopbackUrl::parse("http://127.0.0.1:43123/media/token").expect("a loopback URL");
        let player = MpvPlayer::with_binary("/usr/bin/true");
        player
            .play(PlayerInput::Stream(&url), None, None, Arc::new(Noop))
            .expect("the binary spawns");
        player.stop();
    }

    #[test]
    fn a_file_is_spawned_and_the_previous_child_is_replaced() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let safe = file(&directory);
        // `/usr/bin/true` ignores the flags and exits, which is enough to prove the spawn path and
        // the replacement without a player installed.
        let player = MpvPlayer::with_binary("/usr/bin/true");
        player
            .play(PlayerInput::File(&safe), None, None, Arc::new(Noop))
            .expect("the binary spawns");
        player
            .play(PlayerInput::File(&safe), None, None, Arc::new(Noop))
            .expect("a second press replaces the first");
        player.stop();
    }

    #[test]
    fn a_missing_binary_is_reported_rather_than_silently_doing_nothing() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let media = file(&directory);
        let player = MpvPlayer::with_binary("/nonexistent/peregrine-mpv");
        let error = player
            .play(PlayerInput::File(&media), None, None, Arc::new(Noop))
            .expect_err("no such binary");
        assert_eq!(error, PlayerError::Spawn);
    }
}
