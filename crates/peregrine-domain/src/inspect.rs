//! Torrent inspection: whether a described file set may be written to disk.
//!
//! [`TorrentInspector`] judges a torrent's *description* — the path text, byte
//! size, and link kind of every entry — and returns a [`SafetyReport`]. It is
//! pure. `peregrine-domain` has no filesystem (AGENTS R6), so it never stats a
//! path and cannot see how much space the volume has; the free-space floor enters
//! as the documented default total-size cap ([`InspectionLimits::with_defaults`]),
//! and the acquisition service compares the total plus the floor against the real
//! free space when it resolves the save path (`docs/FLOWS.md` F6).
//!
//! The split with [`PathGuard`] is deliberate, and neither half repeats the other.
//! `PathGuard` owns what path *text* can decide: containment, `..`, drive forms,
//! Windows device names, characters a later layer could reinterpret, and component
//! length. This module owns the policy on top: which extensions may be written,
//! which link kinds may exist at all, how many entries and how many bytes a
//! torrent may describe, and which refusals a user may accept. Every entry is
//! validated by `PathGuard::resolve_relative` before it is classified, so a rule
//! the guard enforces has exactly one implementation.
//!
//! Refusals are [`Reason`]s and the outcome is a [`Verdict`] — never two booleans.
//! "Blocked" and "blocked until the user accepts the advanced risk" are different
//! states of the same download, and telling them apart is the whole control: a
//! `Verdict` cannot be misread the way `allowed == false && overridable == true`
//! can, and no caller can reach the override path by accident.

use serde::{Deserialize, Serialize};

use crate::path::{PathGuard, PathGuardError};
use crate::stored_enum;

// Media a torrent may write: video containers and elementary streams, audio, the
// subtitle formats the player and the subtitle pipeline read, and the sidecars a
// release legitimately ships beside the media. An allowlist, not a blocklist: an
// extension nobody has classified is refused rather than assumed inert
// (SECURITY.md T2).
const ALLOWED_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "webm", "flv", "mpg", "mpeg", "m2ts", "mts", "ts",
    "vob", "ogv", "divx", "3gp", "mp3", "flac", "aac", "m4a", "ogg", "oga", "opus", "wav", "wma",
    "ac3", "eac3", "dts", "mka", "srt", "ass", "ssa", "vtt", "sub", "idx", "sup", "smi", "ttml",
    "nfo", "txt", "md", "cue", "log", "sfv", "jpg", "jpeg", "png", "gif", "webp",
];

// Extensions the operating system launches or interprets. Refused with no
// override (SECURITY.md T2): whatever the release name claims, a download that
// ends in one of these is a program — an installer, a shortcut, a library, or a
// script — and double-clicking it is the user's whole machine.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "com", "bat", "cmd", "msi", "msp", "scr", "pif", "cpl", "hta", "lnk", "url", "reg",
    "inf", "dll", "sys", "so", "dylib", "app", "command", "action", "workflow", "scpt", "terminal",
    "sh", "bash", "zsh", "fish", "ksh", "csh", "py", "pyc", "pyo", "pl", "pm", "rb", "php", "lua",
    "js", "mjs", "jse", "vbs", "vbe", "wsf", "wsh", "ps1", "psm1", "psd1", "jar", "class", "apk",
    "deb", "rpm", "pkg", "run",
];

// Formats whose *contents* never meet this inspector. A nested archive is the
// decompression bomb and the hidden-payload case at once (SECURITY.md T4): what
// is inside it is chosen after the report was written, so it is refused unless
// the user accepts the advanced risk on the record.
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "zipx", "rar", "r00", "7z", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz",
    "zst", "zstd", "lz", "lzma", "lz4", "cab", "arj", "ace", "sit", "sitx",
];

// Disk images. Opening one hands the operating system a second, uninspected
// filesystem with a mount point attached — the same opaque-content problem as an
// archive, so the same advanced opt-in (SECURITY.md T4).
const DISK_IMAGE_EXTENSIONS: &[&str] = &[
    "dmg",
    "iso",
    "img",
    "bin",
    "vhd",
    "vhdx",
    "vmdk",
    "vdi",
    "qcow2",
    "wim",
    "toast",
    "sparseimage",
];

// Bidirectional formatting controls. They are invisible in most fonts and reverse
// the order the following characters render in, so `photo\u{202e}gpj.exe` shows
// as `photoexe.jpg` in any renderer that honours them while the bytes on disk end
// in `.exe` (SECURITY.md T2). They are not in `PathGuard`'s confusable set,
// because they do not decompose into a separator; what they spoof is the whole
// name, which is why they are refused here in any position and whatever the
// extension turns out to be. No release names a media file with one.
const BIDI_CONTROLS: &[char] = &[
    '\u{061c}', // ARABIC LETTER MARK
    '\u{200e}', // LEFT-TO-RIGHT MARK
    '\u{200f}', // RIGHT-TO-LEFT MARK
    '\u{202a}', // LEFT-TO-RIGHT EMBEDDING
    '\u{202b}', // RIGHT-TO-LEFT EMBEDDING
    '\u{202c}', // POP DIRECTIONAL FORMATTING
    '\u{202d}', // LEFT-TO-RIGHT OVERRIDE
    '\u{202e}', // RIGHT-TO-LEFT OVERRIDE
    '\u{2066}', // LEFT-TO-RIGHT ISOLATE
    '\u{2067}', // RIGHT-TO-LEFT ISOLATE
    '\u{2068}', // FIRST STRONG ISOLATE
    '\u{2069}', // POP DIRECTIONAL ISOLATE
];

// Percent-encoded spellings of `.`, `/`, `\`, and NUL, matched in either case.
// Nothing in our write path decodes a path, so today these are ordinary filename
// bytes — which is exactly why they are refused: one URI, one link, or one system
// API that decodes later is all a traversal needs (SECURITY.md T1), and a media
// file whose name contains one is not a case worth risking a user's home
// directory on.
const ENCODED_PATH_TOKENS: &[&str] = &["%2e", "%2f", "%5c", "%00"];

/// What an entry's metadata says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A symlink entry. The link target is written by the torrent engine, so a
    /// path that is inside the root as text can point anywhere once the kernel
    /// follows it (SECURITY.md T3).
    Symlink,
    /// A hardlink entry, which has the same problem with no link to inspect
    /// (SECURITY.md T3).
    Hardlink,
}

/// One file a torrent says it will write.
///
/// Every field is untrusted: this is metadata written by a stranger, so
/// construction validates nothing and [`TorrentInspector`] is what judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentEntry {
    path: String,
    bytes: u64,
    kind: EntryKind,
}

impl TorrentEntry {
    /// Describes one entry.
    pub fn new(path: impl Into<String>, bytes: u64, kind: EntryKind) -> Self {
        Self {
            path: path.into(),
            bytes,
            kind,
        }
    }

    /// The path text as the metadata wrote it, relative to the save path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The size the metadata claims for the entry.
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Whether the metadata describes a file, a symlink, or a hardlink.
    pub const fn kind(&self) -> EntryKind {
        self.kind
    }
}

/// Every file a torrent describes, before a byte is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentDescription {
    entries: Vec<TorrentEntry>,
}

impl TorrentDescription {
    /// Describes a torrent's file set.
    pub fn new(entries: Vec<TorrentEntry>) -> Self {
        Self { entries }
    }

    /// The entries, in the order the metadata listed them. The order is not part
    /// of the judgement.
    pub fn entries(&self) -> &[TorrentEntry] {
        &self.entries
    }

    /// How many entries the metadata describes.
    pub fn file_count(&self) -> usize {
        self.entries.len()
    }

    /// The total size the entries add up to.
    ///
    /// Saturating rather than wrapping: hostile metadata reports sizes near
    /// `u64::MAX`, and a size cap that wrapped would read an enormous torrent as
    /// a tiny one.
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .fold(0, |total, entry| total.saturating_add(entry.bytes()))
    }
}

/// The caps one inspection applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InspectionLimits {
    max_files: usize,
    max_total_bytes: u64,
}

impl InspectionLimits {
    /// Entries a torrent may describe before it is refused.
    pub const DEFAULT_MAX_FILES: usize = 500;

    /// The documented defaults for a volume that reserves `free_space_floor`
    /// bytes: [`DEFAULT_MAX_FILES`](Self::DEFAULT_MAX_FILES) entries and a total
    /// of twice the floor (`docs/ARCHITECTURE.md` §12). The floor is a setting
    /// about the volume, not a property of the torrent, so it is an argument
    /// here rather than a constant.
    pub fn with_defaults(free_space_floor: u64) -> Self {
        Self {
            max_files: Self::DEFAULT_MAX_FILES,
            // A floor near `u64::MAX` is nonsense, but finding that out must not
            // panic a debug build.
            max_total_bytes: free_space_floor.saturating_mul(2),
        }
    }

    /// Explicit caps, for a policy that does not use the documented defaults.
    pub const fn new(max_files: usize, max_total_bytes: u64) -> Self {
        Self {
            max_files,
            max_total_bytes,
        }
    }

    /// The entry cap.
    pub const fn max_files(self) -> usize {
        self.max_files
    }

    /// The total-size cap.
    pub const fn max_total_bytes(self) -> u64 {
        self.max_total_bytes
    }
}

/// The user's answer to the advanced-risk prompt for one torrent (SECURITY.md T4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptIn {
    /// No prompt has been answered, so an advanced-risk finding blocks the
    /// download.
    NotGranted,
    /// The user accepted the risk in an explicit, audited prompt.
    Granted,
}

stored_enum! {
    /// Why an entry or a torrent was refused.
    ///
    /// The spelling is a contract, not a label: it is stored in the report and in
    /// `download_file.blocked_reason`, and the UI turns it into text, so renaming
    /// one is a data migration rather than a copy edit.
    Reason {
        /// A `..` component, or a path that leaves the root once folded (T1).
        PathTraversal => "path_traversal",
        /// An absolute path or a bare drive form (T1).
        AbsolutePath => "absolute_path",
        /// A percent-encoded dot, separator, or NUL: a traversal for whatever
        /// decodes it next (T1).
        PercentEncodedPath => "percent_encoded_path",
        /// A Windows device name in any spelling, including one wearing a media
        /// extension (T1).
        ReservedDeviceName => "reserved_device_name",
        /// A component character the guard refuses: NUL, `\`, `:`, or a
        /// lookalike separator (T1).
        ForbiddenCharacter => "forbidden_character",
        /// A component longer than a filesystem accepts.
        ComponentTooLong => "component_too_long",
        /// An empty, `.`-bearing, or non-UTF-8 entry: not a plain relative file
        /// name.
        MalformedPath => "malformed_path",
        /// The metadata declares a symlink entry (T3).
        SymlinkEntry => "symlink_entry",
        /// The metadata declares a hardlink entry (T3).
        HardlinkEntry => "hardlink_entry",
        /// An extension the operating system launches or interprets (T2).
        ExecutablePayload => "executable_payload",
        /// A bidirectional formatting control, which makes the name render as a
        /// different name (T2).
        UnicodeSpoofing => "unicode_spoofing",
        /// An archive whose contents never met this inspector (T4).
        NestedArchive => "nested_archive",
        /// A disk image, which mounts a second uninspected filesystem (T4).
        DiskImage => "disk_image",
        /// An extension that is absent or not on the allowlist (T2).
        ExtensionNotAllowed => "extension_not_allowed",
        /// More entries than the cap allows (T4).
        FileCountCapped => "file_count_capped",
        /// More bytes than the cap allows (T4).
        TotalSizeCapped => "total_size_capped",
    }
}

impl Reason {
    /// True when an explicit advanced opt-in lets the download proceed anyway.
    ///
    /// A path or executable finding never returns true. That is the line this
    /// module exists to hold: the opt-in is a risk the user may take on opaque
    /// content, not a permission to write outside the root or to land a program.
    pub const fn requires_opt_in(self) -> bool {
        matches!(self, Self::NestedArchive | Self::DiskImage)
    }
}

/// One rule a description broke, and the entry it applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    reason: Reason,
    entry: Option<String>,
}

impl Finding {
    /// The rule that refused the entry.
    pub const fn reason(&self) -> Reason {
        self.reason
    }

    /// The path text of the entry the finding applies to, or `None` when the rule
    /// applies to the torrent as a whole.
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }

    fn torrent(reason: Reason) -> Self {
        Self {
            reason,
            entry: None,
        }
    }

    fn for_entry(reason: Reason, path: &str) -> Self {
        Self {
            reason,
            entry: Some(path.to_owned()),
        }
    }
}

stored_enum! {
    /// What an inspection allows.
    Verdict {
        /// Nothing stands between the description and the disk.
        Allowed => "allowed",
        /// Refused until the user accepts the advanced risk in an explicit,
        /// audited prompt. A grant covers one torrent and nothing else.
        OptInRequired => "opt_in_required",
        /// Refused outright. No setting, acknowledgement, retry, or opt-in
        /// changes it: the entry traverses, links, or is a program.
        Blocked => "blocked",
    }
}

/// The judgement on one torrent's description, before a byte is written.
///
/// Persisted with the job (`download_job.safety_report_json`, invariant I4) so a
/// block can still be explained after a restart without re-fetching metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyReport {
    verdict: Verdict,
    findings: Vec<Finding>,
    file_count: usize,
    total_bytes: u64,
}

impl SafetyReport {
    /// What the inspection allows.
    pub const fn verdict(&self) -> Verdict {
        self.verdict
    }

    /// Every rule the description broke, ordered by the entry path and then by
    /// reason, so equal input sets produce equal reports.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// How many entries were judged.
    pub fn file_count(&self) -> usize {
        self.file_count
    }

    /// The total size the entries added up to.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// Judges a torrent description against one set of limits.
#[derive(Debug, Clone, Copy)]
pub struct TorrentInspector {
    limits: InspectionLimits,
}

impl TorrentInspector {
    /// Builds an inspector with the given caps.
    pub const fn new(limits: InspectionLimits) -> Self {
        Self { limits }
    }

    /// Judges every entry and returns the verdict.
    ///
    /// The opt-in is named at each call rather than held by the inspector. A
    /// grant belongs to one torrent and one prompt; a value that can be built
    /// once and reused is a grant that silently covers the next download.
    pub fn inspect(
        &self,
        description: &TorrentDescription,
        guard: &PathGuard,
        opt_in: OptIn,
    ) -> SafetyReport {
        let file_count = description.file_count();
        let total_bytes = description.total_bytes();
        let mut findings = Vec::new();

        if file_count > self.limits.max_files() {
            findings.push(Finding::torrent(Reason::FileCountCapped));
        }
        if total_bytes > self.limits.max_total_bytes() {
            findings.push(Finding::torrent(Reason::TotalSizeCapped));
        }

        for entry in description.entries() {
            let path = entry.path();
            if let Some(reason) = guard_reason(path, guard) {
                // The documented order is path validation, then extension
                // (`docs/ARCHITECTURE.md` §12). A name the guard refused is not
                // read any further: the refusal a user is shown has to be the
                // dangerous one, not whichever reason happens to sort first, and
                // the entry is blocked either way.
                findings.push(Finding::for_entry(reason, path));
                continue;
            }
            if has_encoded_path_token(path) {
                findings.push(Finding::for_entry(Reason::PercentEncodedPath, path));
            }
            if let Some(reason) = link_reason(entry.kind()) {
                findings.push(Finding::for_entry(reason, path));
            }
            if let Some(reason) = extension_reason(path) {
                findings.push(Finding::for_entry(reason, path));
            }
            if has_bidi_control(path) {
                findings.push(Finding::for_entry(Reason::UnicodeSpoofing, path));
            }
        }

        // Input order is not part of the answer: two torrents describing the same
        // entries must produce the same report, finding for finding, so the report
        // is ordered by the entry it names and then by rule.
        findings.sort_unstable_by(|left, right| {
            left.entry
                .cmp(&right.entry)
                .then_with(|| left.reason.as_str().cmp(right.reason.as_str()))
        });

        SafetyReport {
            verdict: verdict_of(&findings, opt_in),
            findings,
            file_count,
            total_bytes,
        }
    }
}

/// The verdict the findings imply, given the user's answer.
fn verdict_of(findings: &[Finding], opt_in: OptIn) -> Verdict {
    if findings
        .iter()
        .any(|finding| !finding.reason().requires_opt_in())
    {
        return Verdict::Blocked;
    }
    if findings
        .iter()
        .any(|finding| finding.reason().requires_opt_in())
        && matches!(opt_in, OptIn::NotGranted)
    {
        return Verdict::OptInRequired;
    }
    Verdict::Allowed
}

/// The reason `PathGuard` refused the entry, or `None` when it accepted it.
fn guard_reason(path: &str, guard: &PathGuard) -> Option<Reason> {
    let error = guard.resolve_relative(path).err()?;
    Some(match error {
        PathGuardError::EscapesRoot
        | PathGuardError::ParentComponent
        | PathGuardError::OutsideRoot => Reason::PathTraversal,
        PathGuardError::AbsoluteEntry | PathGuardError::DriveForm => Reason::AbsolutePath,
        PathGuardError::ReservedName { .. } => Reason::ReservedDeviceName,
        // The guard names the character, never the intent: a backslash is an
        // ordinary byte to Unix and a separator to Windows, and the text alone
        // cannot say which the writer meant. Every forbidden character blocks,
        // so the distinction is only how the refusal is explained.
        PathGuardError::ForbiddenCharacter { .. } => Reason::ForbiddenCharacter,
        PathGuardError::ComponentTooLong { .. } => Reason::ComponentTooLong,
        // An empty component or a `.` component is not a plain relative file
        // name. The three below it cannot come out of `resolve_relative`: it only
        // ever sees a `&str`, so every component is UTF-8, and the guard was
        // already built by the caller. They are named rather than caught by a
        // wildcard so that a new guard error fails this build instead of quietly
        // acquiring a policy, and a path no rule describes is refused rather than
        // interpreted.
        PathGuardError::EmptyComponent
        | PathGuardError::NotCanonical
        | PathGuardError::InvalidEncoding
        | PathGuardError::RootNotAbsolute
        | PathGuardError::RootNotCanonical => Reason::MalformedPath,
    })
}

/// The reason an entry's link kind is refused, or `None` for a regular file.
fn link_reason(kind: EntryKind) -> Option<Reason> {
    match kind {
        EntryKind::File => None,
        EntryKind::Symlink => Some(Reason::SymlinkEntry),
        EntryKind::Hardlink => Some(Reason::HardlinkEntry),
    }
}

fn has_encoded_path_token(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    ENCODED_PATH_TOKENS
        .iter()
        .any(|token| lowered.contains(token))
}

fn has_bidi_control(path: &str) -> bool {
    path.chars()
        .any(|character| BIDI_CONTROLS.contains(&character))
}

/// The reason an entry's extension is refused, or `None` when it is allowed.
fn extension_reason(path: &str) -> Option<Reason> {
    match class_of(extension_of(path)) {
        ExtensionClass::Executable => Some(Reason::ExecutablePayload),
        ExtensionClass::Archive => Some(Reason::NestedArchive),
        ExtensionClass::DiskImage => Some(Reason::DiskImage),
        ExtensionClass::Allowed => None,
        ExtensionClass::Unknown => Some(Reason::ExtensionNotAllowed),
    }
}

/// What policy says about one extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionClass {
    Executable,
    Archive,
    DiskImage,
    Allowed,
    Unknown,
}

/// The extension of the last path component, without its leading dot.
///
/// Trailing spaces and dots are stripped first, because Windows ignores them when
/// it resolves a name: `movie.mkv.exe.` is an executable there, and a name that
/// ends in a space is the media file it looks like. Reading only the text after
/// the last dot would call the first one extensionless and the second one
/// unknown.
fn extension_of(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.trim_end_matches([' ', '.']);
    let (_, extension) = name.rsplit_once('.')?;
    (!extension.is_empty()).then_some(extension)
}

fn class_of(extension: Option<&str>) -> ExtensionClass {
    match extension {
        Some(extension) if in_list(EXECUTABLE_EXTENSIONS, extension) => ExtensionClass::Executable,
        Some(extension) if in_list(ARCHIVE_EXTENSIONS, extension) => ExtensionClass::Archive,
        Some(extension) if in_list(DISK_IMAGE_EXTENSIONS, extension) => ExtensionClass::DiskImage,
        Some(extension) if in_list(ALLOWED_EXTENSIONS, extension) => ExtensionClass::Allowed,
        // Absent, or present and unclassified. Both are refused: an allowlist
        // that admits what it does not recognise is a blocklist.
        _ => ExtensionClass::Unknown,
    }
}

fn in_list(list: &[&str], extension: &str) -> bool {
    list.iter().any(|item| item.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use super::*;
    use crate::path::MAX_COMPONENT_BYTES;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\downloads")
        } else {
            PathBuf::from("/downloads")
        }
    }

    fn guard() -> PathGuard {
        PathGuard::new(root()).expect("a canonical absolute root")
    }

    fn file(path: &str, bytes: u64) -> TorrentEntry {
        TorrentEntry::new(path, bytes, EntryKind::File)
    }

    fn link(path: &str, kind: EntryKind) -> TorrentEntry {
        TorrentEntry::new(path, 1, kind)
    }

    /// Caps wide enough that only the rule under test can fire.
    fn uncapped() -> TorrentInspector {
        TorrentInspector::new(InspectionLimits::new(1_000_000, u64::MAX))
    }

    fn inspected(entries: Vec<TorrentEntry>, opt_in: OptIn) -> SafetyReport {
        uncapped().inspect(&TorrentDescription::new(entries), &guard(), opt_in)
    }

    fn findings(report: &SafetyReport) -> Vec<(Reason, Option<&str>)> {
        report
            .findings()
            .iter()
            .map(|finding| (finding.reason(), finding.entry()))
            .collect()
    }

    /// A description of `count` allowlisted entries of one byte each.
    fn many(count: usize) -> TorrentDescription {
        TorrentDescription::new(
            (0..count)
                .map(|index| file(&format!("file-{index:04}.mkv"), 1))
                .collect(),
        )
    }

    #[test]
    fn a_parent_component_is_blocked_as_traversal() {
        let report = inspected(vec![file("../authorized_keys", 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::PathTraversal, Some("../authorized_keys"))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn a_traversal_that_would_stay_inside_is_still_refused() {
        // `PathGuard::resolve` folds this back inside the root, but an entry may
        // not carry `..` at all: the engine assembles the final path, and this
        // module never sees the result.
        let report = inspected(vec![file("extras/../movie.mkv", 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::PathTraversal, Some("extras/../movie.mkv"))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn an_absolute_entry_is_blocked() {
        let report = inspected(vec![file("/etc/passwd", 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::AbsolutePath, Some("/etc/passwd"))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn a_windows_path_is_blocked_on_a_host_without_drive_letters() {
        let path = r"C:\Users\Public\payload.mkv";
        let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::ForbiddenCharacter, Some(path))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn a_percent_encoded_traversal_is_blocked() {
        let path = "..%2f..%2fetc%2fpasswd.mkv";
        let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::PercentEncodedPath, Some(path))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn percent_encoding_is_refused_in_any_case() {
        for path in [
            "a%2Fb.mkv",
            "a%2fb.mkv",
            "a%2Eb.mkv",
            "a%00b.mkv",
            "a%5Cb.mkv",
        ] {
            let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
            assert_eq!(
                findings(&report),
                [(Reason::PercentEncodedPath, Some(path))],
                "`{path}`"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "`{path}`");
        }
    }

    #[test]
    fn a_reserved_device_name_is_blocked_in_any_spelling() {
        for path in [
            "CON.mkv",
            "con .mkv",
            "nul.txt",
            "com1.mp4",
            "LPT1",
            "extras/NUL/report.nfo",
        ] {
            let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
            assert_eq!(
                findings(&report),
                [(Reason::ReservedDeviceName, Some(path))],
                "`{path}`"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "`{path}`");
        }
    }

    #[test]
    fn a_backslash_is_refused_as_a_component_character() {
        let path = r"..\..\evil.mkv";
        let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::ForbiddenCharacter, Some(path))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn a_lookalike_separator_is_refused() {
        let path = "a\u{ff0f}b.mkv";
        let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::ForbiddenCharacter, Some(path))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn a_nul_byte_is_refused() {
        let path = "a\0b.mkv";
        let report = inspected(vec![file(path, 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&report),
            [(Reason::ForbiddenCharacter, Some(path))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn an_entry_exactly_at_the_component_limit_is_accepted_and_one_byte_above_is_not() {
        let at_limit = format!("{}.mkv", "a".repeat(MAX_COMPONENT_BYTES - 4));
        assert_eq!(at_limit.len(), MAX_COMPONENT_BYTES);
        let accepted = inspected(vec![file(&at_limit, 10)], OptIn::NotGranted);
        assert_eq!(accepted.findings(), []);
        assert_eq!(accepted.verdict(), Verdict::Allowed);

        let above = format!("{}.mkv", "a".repeat(MAX_COMPONENT_BYTES - 3));
        let refused = inspected(vec![file(&above, 10)], OptIn::NotGranted);
        assert_eq!(
            findings(&refused),
            [(Reason::ComponentTooLong, Some(above.as_str()))]
        );
        assert_eq!(refused.verdict(), Verdict::Blocked);
    }

    #[test]
    fn an_empty_entry_path_is_malformed() {
        let report = inspected(vec![file("", 10)], OptIn::NotGranted);
        assert_eq!(findings(&report), [(Reason::MalformedPath, Some(""))]);
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn an_empty_and_a_dot_component_are_malformed() {
        let report = inspected(
            vec![file("a//b.mkv", 10), file("a/./b.mkv", 10)],
            OptIn::NotGranted,
        );
        assert_eq!(
            findings(&report),
            [
                (Reason::MalformedPath, Some("a/./b.mkv")),
                (Reason::MalformedPath, Some("a//b.mkv")),
            ]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn an_entry_without_an_extension_is_refused_by_the_allowlist() {
        let report = inspected(vec![file("README", 10)], OptIn::Granted);
        assert_eq!(
            findings(&report),
            [(Reason::ExtensionNotAllowed, Some("README"))]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn a_symlink_entry_is_blocked_on_every_platform_and_never_overridden() {
        for opt_in in [OptIn::NotGranted, OptIn::Granted] {
            let report = inspected(vec![link("season.mkv", EntryKind::Symlink)], opt_in);
            assert_eq!(
                findings(&report),
                [(Reason::SymlinkEntry, Some("season.mkv"))],
                "{opt_in:?}"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "{opt_in:?}");
        }
    }

    #[test]
    fn a_hardlink_entry_is_blocked_on_every_platform_and_never_overridden() {
        for opt_in in [OptIn::NotGranted, OptIn::Granted] {
            let report = inspected(vec![link("season.mkv", EntryKind::Hardlink)], opt_in);
            assert_eq!(
                findings(&report),
                [(Reason::HardlinkEntry, Some("season.mkv"))],
                "{opt_in:?}"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "{opt_in:?}");
        }
    }

    #[test]
    fn an_executable_or_script_is_blocked_with_no_override() {
        // Judged with the opt-in already granted: the strongest form of the
        // claim, and the one a caller must not be able to argue around.
        for path in [
            "setup.exe",
            "run.bat",
            "install.cmd",
            "payload.msi",
            "flash.scr",
            "evil.jar",
            "hook.sh",
            "tool.ps1",
            "app.app",
            "x.py",
            "mac.command",
        ] {
            let report = inspected(vec![file(path, 10)], OptIn::Granted);
            assert_eq!(
                findings(&report),
                [(Reason::ExecutablePayload, Some(path))],
                "`{path}`"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "`{path}`");
        }
    }

    #[test]
    fn a_double_extension_is_classified_by_its_true_extension() {
        for path in ["movie.mkv.exe", "movie.mkv.exe.", "movie.MKV.EXE"] {
            let report = inspected(vec![file(path, 10)], OptIn::Granted);
            assert_eq!(
                findings(&report),
                [(Reason::ExecutablePayload, Some(path))],
                "`{path}`"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "`{path}`");
        }
    }

    #[test]
    fn a_bidirectional_control_is_blocked_whatever_it_hides() {
        for control in BIDI_CONTROLS {
            for extension in ["exe", "mkv"] {
                let path = format!("holiday{control}gpj.{extension}");
                let spoof = (Reason::UnicodeSpoofing, Some(path.as_str()));
                // The spoofed executable is an executable as well as a lie: the
                // name is refused for rendering as another name, and the
                // extension is refused for being a program.
                let expected = if extension == "exe" {
                    vec![(Reason::ExecutablePayload, Some(path.as_str())), spoof]
                } else {
                    vec![spoof]
                };
                let report = inspected(vec![file(&path, 10)], OptIn::Granted);
                assert_eq!(
                    findings(&report),
                    expected,
                    "U+{:04X} in `{path}`",
                    u32::from(*control)
                );
                assert_eq!(report.verdict(), Verdict::Blocked, "{path}");
            }
        }
    }

    #[test]
    fn a_homoglyph_extension_is_not_on_the_allowlist() {
        // Cyrillic `а` and `в`: these render as `movie.m4a` and `movie.mkv`.
        let report = inspected(
            vec![file("movie.m4\u{430}", 10), file("movie.mk\u{432}", 10)],
            OptIn::NotGranted,
        );
        assert_eq!(
            findings(&report),
            [
                (Reason::ExtensionNotAllowed, Some("movie.m4\u{430}")),
                (Reason::ExtensionNotAllowed, Some("movie.mk\u{432}")),
            ]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn an_archive_requires_the_advanced_opt_in() {
        for path in [
            "extras.zip",
            "extras.7z",
            "extras.rar",
            "extras.tar.gz",
            "extras.cab",
        ] {
            let refused = inspected(vec![file(path, 10)], OptIn::NotGranted);
            assert_eq!(
                findings(&refused),
                [(Reason::NestedArchive, Some(path))],
                "`{path}`"
            );
            assert_eq!(refused.verdict(), Verdict::OptInRequired, "`{path}`");

            let accepted = inspected(vec![file(path, 10)], OptIn::Granted);
            assert_eq!(
                findings(&accepted),
                [(Reason::NestedArchive, Some(path))],
                "`{path}`"
            );
            assert_eq!(accepted.verdict(), Verdict::Allowed, "`{path}`");
        }
    }

    #[test]
    fn a_disk_image_requires_the_advanced_opt_in() {
        for path in ["image.iso", "image.dmg", "image.img", "image.vhd"] {
            let refused = inspected(vec![file(path, 10)], OptIn::NotGranted);
            assert_eq!(
                findings(&refused),
                [(Reason::DiskImage, Some(path))],
                "`{path}`"
            );
            assert_eq!(refused.verdict(), Verdict::OptInRequired, "`{path}`");

            let accepted = inspected(vec![file(path, 10)], OptIn::Granted);
            assert_eq!(accepted.verdict(), Verdict::Allowed, "`{path}`");
        }
    }

    #[test]
    fn an_extension_outside_the_allowlist_is_blocked_with_no_override() {
        for path in ["payload.xyz", "payload.dat", "movie.mkv.bak", "notes.pdf"] {
            let report = inspected(vec![file(path, 10)], OptIn::Granted);
            assert_eq!(
                findings(&report),
                [(Reason::ExtensionNotAllowed, Some(path))],
                "`{path}`"
            );
            assert_eq!(report.verdict(), Verdict::Blocked, "`{path}`");
        }
    }

    #[test]
    fn an_allowed_torrent_produces_no_findings() {
        let report = inspected(
            vec![
                file("Show/Season 01/S01E01.mkv", 1_000),
                file("Show/Season 01/S01E01.srt", 1),
                file("Show/Season 01/poster.jpg", 1),
                file("Show/Show.nfo", 1),
                file("Show/Show - S01E01.mp3", 1),
            ],
            OptIn::NotGranted,
        );
        assert_eq!(report.findings(), []);
        assert_eq!(report.verdict(), Verdict::Allowed);
        assert_eq!(report.file_count(), 5);
        assert_eq!(report.total_bytes(), 1_004);
    }

    #[test]
    fn a_non_ascii_media_name_is_accepted() {
        let report = inspected(
            vec![
                file("Amélie (2001)/Amélie.mkv", 10),
                file("Amélie (2001)/Amélie.srt", 10),
            ],
            OptIn::NotGranted,
        );
        assert_eq!(report.findings(), []);
        assert_eq!(report.verdict(), Verdict::Allowed);
    }

    #[test]
    fn an_empty_torrent_is_allowed_with_no_findings() {
        // No files is not a safety finding: nothing would be written, and the add
        // path refuses a job with nothing to download.
        let report = inspected(Vec::new(), OptIn::NotGranted);
        assert_eq!(report.findings(), []);
        assert_eq!(report.verdict(), Verdict::Allowed);
        assert_eq!(report.file_count(), 0);
        assert_eq!(report.total_bytes(), 0);
    }

    #[test]
    fn a_repeated_hostile_entry_is_reported_once_per_copy() {
        let report = inspected(
            vec![file("../evil.mkv", 10), file("../evil.mkv", 10)],
            OptIn::NotGranted,
        );
        assert_eq!(
            findings(&report),
            [
                (Reason::PathTraversal, Some("../evil.mkv")),
                (Reason::PathTraversal, Some("../evil.mkv")),
            ]
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
    }

    #[test]
    fn the_file_count_cap_defaults_to_500_and_admits_exactly_500() {
        let limits = InspectionLimits::with_defaults(1_000_000);
        assert_eq!(limits.max_files(), InspectionLimits::DEFAULT_MAX_FILES);
        assert_eq!(limits.max_files(), 500);

        let inspector = TorrentInspector::new(limits);
        let at_cap = inspector.inspect(&many(500), &guard(), OptIn::NotGranted);
        assert_eq!(at_cap.findings(), []);
        assert_eq!(at_cap.verdict(), Verdict::Allowed);
        assert_eq!(at_cap.file_count(), 500);

        let above_cap = inspector.inspect(&many(501), &guard(), OptIn::NotGranted);
        assert_eq!(findings(&above_cap), [(Reason::FileCountCapped, None)]);
        assert_eq!(above_cap.verdict(), Verdict::Blocked);
        assert_eq!(above_cap.file_count(), 501);
    }

    #[test]
    fn an_explicit_file_count_cap_admits_exactly_the_cap() {
        let inspector = TorrentInspector::new(InspectionLimits::new(2, u64::MAX));
        assert_eq!(
            inspector
                .inspect(&many(2), &guard(), OptIn::NotGranted)
                .verdict(),
            Verdict::Allowed
        );
        assert_eq!(
            findings(&inspector.inspect(&many(3), &guard(), OptIn::NotGranted)),
            [(Reason::FileCountCapped, None)]
        );
    }

    #[test]
    fn the_total_size_cap_defaults_to_twice_the_floor_and_admits_exactly_the_cap() {
        let limits = InspectionLimits::with_defaults(1_000);
        assert_eq!(limits.max_total_bytes(), 2_000);

        let inspector = TorrentInspector::new(limits);
        let at_cap = inspector.inspect(
            &TorrentDescription::new(vec![file("movie.mkv", 2_000)]),
            &guard(),
            OptIn::NotGranted,
        );
        assert_eq!(at_cap.findings(), []);
        assert_eq!(at_cap.verdict(), Verdict::Allowed);
        assert_eq!(at_cap.total_bytes(), 2_000);

        let above_cap = inspector.inspect(
            &TorrentDescription::new(vec![file("movie.mkv", 2_001)]),
            &guard(),
            OptIn::NotGranted,
        );
        assert_eq!(findings(&above_cap), [(Reason::TotalSizeCapped, None)]);
        assert_eq!(above_cap.verdict(), Verdict::Blocked);
        assert_eq!(above_cap.total_bytes(), 2_001);
    }

    #[test]
    fn an_explicit_total_size_cap_admits_exactly_the_cap() {
        let inspector = TorrentInspector::new(InspectionLimits::new(10, 1_000));
        let at_cap = inspector.inspect(
            &TorrentDescription::new(vec![file("movie.mkv", 1_000)]),
            &guard(),
            OptIn::NotGranted,
        );
        assert_eq!(at_cap.verdict(), Verdict::Allowed);
        let above_cap = inspector.inspect(
            &TorrentDescription::new(vec![file("movie.mkv", 1_001)]),
            &guard(),
            OptIn::NotGranted,
        );
        assert_eq!(findings(&above_cap), [(Reason::TotalSizeCapped, None)]);
    }

    #[test]
    fn an_enormous_reported_size_saturates_rather_than_overflowing() {
        let description =
            TorrentDescription::new(vec![file("a.mkv", u64::MAX), file("b.mkv", u64::MAX)]);
        assert_eq!(description.total_bytes(), u64::MAX);
        let report = TorrentInspector::new(InspectionLimits::new(10, u64::MAX)).inspect(
            &description,
            &guard(),
            OptIn::NotGranted,
        );
        assert_eq!(report.findings(), []);
        assert_eq!(report.verdict(), Verdict::Allowed);
        assert_eq!(
            InspectionLimits::with_defaults(u64::MAX).max_total_bytes(),
            u64::MAX
        );
    }

    #[test]
    fn an_advanced_risk_needs_the_opt_in_and_then_passes() {
        let entries = vec![file("extras.zip", 10)];
        let refused = inspected(entries.clone(), OptIn::NotGranted);
        assert_eq!(refused.verdict(), Verdict::OptInRequired);

        let accepted = inspected(entries, OptIn::Granted);
        assert_eq!(accepted.verdict(), Verdict::Allowed);
        assert_eq!(
            accepted.findings(),
            refused.findings(),
            "the findings do not depend on the opt-in, only the verdict does"
        );
    }

    #[test]
    fn a_hard_block_is_never_cleared_by_the_advanced_opt_in() {
        // One entry traverses, one is a nested archive, one is a program. The
        // archive is overridable on its own, and it still cannot drag the
        // torrent through.
        let entries = vec![
            file("../escape.mkv", 10),
            file("extras.zip", 10),
            file("setup.exe", 10),
        ];
        for opt_in in [OptIn::NotGranted, OptIn::Granted] {
            let report = inspected(entries.clone(), opt_in);
            assert_eq!(report.verdict(), Verdict::Blocked, "{opt_in:?}");
            assert_eq!(
                findings(&report),
                [
                    (Reason::PathTraversal, Some("../escape.mkv")),
                    (Reason::NestedArchive, Some("extras.zip")),
                    (Reason::ExecutablePayload, Some("setup.exe")),
                ],
                "{opt_in:?}"
            );
        }
    }

    #[test]
    fn the_order_of_entries_does_not_change_the_report() {
        let entries = vec![
            file("extras.zip", 10),
            file("../escape.mkv", 10),
            file("show/S01E01.mkv", 100),
            file("setup.exe", 10),
            link("link.mkv", EntryKind::Symlink),
        ];
        let forward = inspected(entries.clone(), OptIn::NotGranted);
        let mut reversed_entries = entries;
        reversed_entries.reverse();
        let reversed = inspected(reversed_entries, OptIn::NotGranted);
        assert_eq!(forward, reversed);
    }

    #[test]
    fn reason_strings_are_stable_and_match_their_serialized_spelling() {
        for reason in Reason::ALL {
            let json = serde_json::to_string(reason).expect("a reason serializes");
            assert_eq!(json, format!("\"{}\"", reason.as_str()), "{reason:?}");
            assert_eq!(Reason::parse(reason.as_str()), Some(*reason), "{reason:?}");
        }
        for verdict in Verdict::ALL {
            let json = serde_json::to_string(verdict).expect("a verdict serializes");
            assert_eq!(json, format!("\"{}\"", verdict.as_str()), "{verdict:?}");
        }
    }

    #[test]
    fn only_the_advanced_risks_may_be_overridden() {
        let overridable: Vec<Reason> = Reason::ALL
            .iter()
            .copied()
            .filter(|reason| reason.requires_opt_in())
            .collect();
        assert_eq!(overridable, [Reason::NestedArchive, Reason::DiskImage]);
    }

    #[test]
    fn a_report_round_trips_through_json() {
        let report = inspected(
            vec![file("../escape.mkv", 10), file("extras.zip", 10)],
            OptIn::NotGranted,
        );
        assert_eq!(report.verdict(), Verdict::Blocked);
        let json = serde_json::to_string(&report).expect("a report serializes");
        assert!(json.contains("\"path_traversal\""), "{json}");
        assert!(json.contains("\"nested_archive\""), "{json}");
        assert!(json.contains("\"blocked\""), "{json}");
        let parsed: SafetyReport = serde_json::from_str(&json).expect("a report deserializes");
        assert_eq!(parsed, report);
    }

    fn path_strategy() -> impl Strategy<Value = String> {
        prop_oneof![any::<String>(), r"[a-zA-Z0-9 ._/\\%#-]{0,24}"]
    }

    fn kind_strategy() -> impl Strategy<Value = EntryKind> {
        prop::sample::select(vec![
            EntryKind::File,
            EntryKind::Symlink,
            EntryKind::Hardlink,
        ])
    }

    fn entry_strategy() -> impl Strategy<Value = TorrentEntry> {
        (path_strategy(), any::<u64>(), kind_strategy())
            .prop_map(|(path, bytes, kind)| TorrentEntry::new(path, bytes, kind))
    }

    fn description_strategy() -> impl Strategy<Value = TorrentDescription> {
        prop::collection::vec(entry_strategy(), 0..6).prop_map(TorrentDescription::new)
    }

    fn limits_strategy() -> impl Strategy<Value = InspectionLimits> {
        (0_usize..6, 0_u64..32).prop_map(|(max_files, max_total_bytes)| {
            InspectionLimits::new(max_files, max_total_bytes)
        })
    }

    fn opt_in_strategy() -> impl Strategy<Value = OptIn> {
        prop::sample::select(vec![OptIn::NotGranted, OptIn::Granted])
    }

    proptest! {
        #[test]
        fn inspecting_any_description_never_panics(
            description in description_strategy(),
            limits in limits_strategy(),
            opt_in in opt_in_strategy(),
        ) {
            let _ = TorrentInspector::new(limits).inspect(&description, &guard(), opt_in);
        }

        #[test]
        fn the_same_description_always_produces_the_same_report(
            description in description_strategy(),
            limits in limits_strategy(),
            opt_in in opt_in_strategy(),
        ) {
            let inspector = TorrentInspector::new(limits);
            let first = inspector.inspect(&description, &guard(), opt_in);
            let second = inspector.inspect(&description, &guard(), opt_in);
            prop_assert_eq!(&first, &second);
            for pair in first.findings().windows(2) {
                prop_assert!(
                    (pair[0].entry(), pair[0].reason().as_str())
                        <= (pair[1].entry(), pair[1].reason().as_str())
                );
            }
        }

        #[test]
        fn the_order_of_entries_never_changes_the_report(
            pairs in prop::collection::vec((entry_strategy(), any::<u64>()), 0..6),
            limits in limits_strategy(),
            opt_in in opt_in_strategy(),
        ) {
            let inspector = TorrentInspector::new(limits);
            let listed = TorrentDescription::new(
                pairs.iter().map(|(entry, _)| entry.clone()).collect(),
            );
            let mut keyed = pairs;
            keyed.sort_by_key(|(_, key)| *key);
            let shuffled = TorrentDescription::new(
                keyed.into_iter().map(|(entry, _)| entry).collect(),
            );
            prop_assert_eq!(
                inspector.inspect(&listed, &guard(), opt_in),
                inspector.inspect(&shuffled, &guard(), opt_in),
            );
        }

        #[test]
        fn an_opt_in_never_clears_a_hard_finding(
            description in description_strategy(),
            limits in limits_strategy(),
        ) {
            let inspector = TorrentInspector::new(limits);
            let refused = inspector.inspect(&description, &guard(), OptIn::NotGranted);
            let accepted = inspector.inspect(&description, &guard(), OptIn::Granted);
            prop_assert_eq!(refused.findings(), accepted.findings());
            if refused.verdict() == Verdict::Blocked {
                prop_assert_eq!(accepted.verdict(), Verdict::Blocked);
            }
            if accepted.verdict() == Verdict::Allowed {
                prop_assert!(
                    accepted
                        .findings()
                        .iter()
                        .all(|finding| finding.reason().requires_opt_in())
                );
            }
            if accepted
                .findings()
                .iter()
                .any(|finding| !finding.reason().requires_opt_in())
            {
                prop_assert_eq!(accepted.verdict(), Verdict::Blocked);
            }
        }
    }
}
