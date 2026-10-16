//! The shipping free-space adapter for `VolumeSpace`.
//!
//! The port's own doc (`crate::acquisition`) records why this cannot be FFI here: every crate in
//! this tree is `#![forbid(unsafe_code)]`, and a native free-space read is `statvfs` or
//! `GetDiskFreeSpaceEx`. This adapter therefore asks the platform's own tool — `df -kP` on Unix,
//! PowerShell's `PSDrive.Free` on Windows — and fails closed (`SpaceError::Denied`) when the tool
//! is missing or its answer cannot be parsed. A native adapter in `peregrine-platform` can replace
//! it without touching a caller, which is the whole reason the port is a trait.

use std::path::Path;
use std::process::Command;

use crate::acquisition::{SpaceError, VolumeSpace};

/// Free space read from the platform's own disk tool.
pub struct SystemVolume;

impl VolumeSpace for SystemVolume {
    fn available_bytes(&self, path: &Path) -> Result<u64, SpaceError> {
        // The destination folder is created by the add, so a path that does not exist yet is the
        // normal case, not an error: the volume the release will land on is the nearest ancestor
        // that does exist. Measuring the folder itself answered `Unreachable` and refused every
        // acquire before it could start.
        let mut probe = path;
        while !probe.exists() {
            match probe.parent() {
                Some(parent) => probe = parent,
                None => return Err(SpaceError::Unreachable),
            }
        }
        available_via_platform(probe).ok_or(SpaceError::Denied)
    }
}

#[cfg(unix)]
fn available_via_platform(path: &Path) -> Option<u64> {
    // `-P` is the POSIX output format, so a long device name does not wrap the columns and the
    // available-bytes column stays the fourth. The path is an argument, never a shell word.
    let output = Command::new("df").arg("-kP").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let row = text.lines().nth(1)?;
    let kib = row.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(windows)]
fn available_via_platform(path: &Path) -> Option<u64> {
    // The path travels in the environment rather than in the script, so a quote in a folder name
    // cannot become PowerShell.
    let script =
        "$d = (Get-Item -LiteralPath $env:PEREGRINE_VOLUME_PATH).PSDrive; if ($d) { $d.Free }";
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("PEREGRINE_VOLUME_PATH", path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(not(any(unix, windows)))]
fn available_via_platform(_path: &Path) -> Option<u64> {
    None
}
