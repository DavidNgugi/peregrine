//! Path containment policy.
//!
//! [`PathGuard`] decides only what path text can decide: whether a candidate
//! stays inside a configured root after `.` and `..` are folded, and whether a
//! component is one that a later layer could reinterpret — a Windows device name,
//! a drive prefix, a separator written as some other character, a NUL, a name past
//! the filesystem limit, or bytes that are not UTF-8. It never touches the
//! filesystem.
//!
//! This check is not the whole control, and the adapter (`PG-E24-03`) still owns
//! the rest: it must resolve real paths with canonicalisation before they reach
//! [`PathGuard`], resolve symlinks and hardlinks itself because a path can be
//! inside the root in text and outside it once the kernel follows a link, and
//! revalidate between planning and confirming a write because the filesystem can
//! change in between (`PG-E15-04`). [`SafePath`] is the token the filesystem layer
//! accepts, and only this module builds one.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// The longest path component a filesystem is expected to accept.
pub const MAX_COMPONENT_BYTES: usize = 255;

// Windows strips trailing spaces and dots, then resolves the stem before the
// first dot, so these are device names in any of those spellings. They are refused
// on every platform rather than only on Windows: a downloaded file or a library
// scanned on Linux can be read on Windows, where the same name becomes a device.
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

// Characters that Unicode compatibility normalisation (NFKC) folds to `/`, `\`, or
// `.`, plus the visually identical slash glyphs that have no decomposition. A
// normalising filesystem, a torrent engine, or a Windows API reached later can turn
// one of these into a separator after this check has passed, so they are refused
// here where the path is still text.
const CONFUSABLE_SEPARATORS: &[char] = &[
    '\u{2044}', // FRACTION SLASH
    '\u{2215}', // DIVISION SLASH
    '\u{ff0f}', // FULLWIDTH SOLIDUS
    '\u{29f8}', // BIG SOLIDUS
    '\u{2571}', // BOX DRAWINGS LIGHT DIAGONAL
    '\u{fe68}', // SMALL REVERSE SOLIDUS
    '\u{ff3c}', // FULLWIDTH REVERSE SOLIDUS
    '\u{2216}', // SET MINUS
    '\u{2572}', // BOX DRAWINGS LIGHT DIAGONAL, reversed
    '\u{2024}', // ONE DOT LEADER
    '\u{ff0e}', // FULLWIDTH FULL STOP
    '\u{2025}', // TWO DOT LEADER
    '\u{2026}', // HORIZONTAL ELLIPSIS
];

/// A configured absolute root that candidate paths must stay inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathGuard {
    root: PathBuf,
    parts: Vec<String>,
    prefix: Option<String>,
}

impl PathGuard {
    /// Builds a guard for an absolute, canonical root.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, PathGuardError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(PathGuardError::RootNotAbsolute);
        }
        let mut parts = Vec::new();
        let mut prefix = None;
        for component in root.components() {
            match component {
                Component::Prefix(value) => prefix = Some(text_of(value.as_os_str())?),
                Component::RootDir => {}
                Component::CurDir | Component::ParentDir => {
                    return Err(PathGuardError::RootNotCanonical);
                }
                Component::Normal(name) => {
                    let text = name.to_str().ok_or(PathGuardError::InvalidEncoding)?;
                    check_component(text)?;
                    parts.push(text.to_owned());
                }
            }
        }
        Ok(Self {
            root,
            parts,
            prefix,
        })
    }

    /// The configured root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// True when a candidate is inside the root and every component passes the policy.
    pub fn contains(&self, candidate: &Path) -> bool {
        self.resolve(candidate).is_ok()
    }

    /// Resolves an absolute or root-relative candidate, folding `.` and `..`, or
    /// names the rule that refused it.
    pub fn resolve(&self, candidate: &Path) -> Result<SafePath, PathGuardError> {
        if candidate.as_os_str().is_empty() {
            return Err(PathGuardError::EmptyComponent);
        }
        let mut absolute = false;
        let mut floor = self.parts.len();
        let mut parts = self.parts.clone();
        let mut prefix = None;
        for component in candidate.components() {
            match component {
                Component::Prefix(value) => prefix = Some(text_of(value.as_os_str())?),
                Component::RootDir => {
                    absolute = true;
                    floor = 0;
                    parts.clear();
                }
                Component::CurDir => return Err(PathGuardError::NotCanonical),
                Component::ParentDir => {
                    if parts.len() <= floor {
                        return Err(PathGuardError::EscapesRoot);
                    }
                    parts.pop();
                }
                Component::Normal(name) => {
                    let text = name.to_str().ok_or(PathGuardError::InvalidEncoding)?;
                    check_component(text)?;
                    parts.push(text.to_owned());
                }
            }
        }
        if prefix.is_some() && !absolute {
            return Err(PathGuardError::DriveForm);
        }
        if absolute
            && (!prefixes_agree(prefix.as_deref(), self.prefix.as_deref())
                || !parts.starts_with(&self.parts))
        {
            return Err(PathGuardError::OutsideRoot);
        }
        Ok(SafePath(self.assemble(&parts)))
    }

    /// Resolves a relative entry name from an untrusted source, treating only `/`
    /// as a separator and refusing `.` and `..` outright.
    pub fn resolve_relative(&self, candidate: &str) -> Result<SafePath, PathGuardError> {
        if candidate.is_empty() {
            return Err(PathGuardError::EmptyComponent);
        }
        if candidate.starts_with('/') {
            return Err(PathGuardError::AbsoluteEntry);
        }
        let mut parts = self.parts.clone();
        for component in candidate.split('/') {
            match component {
                "." => return Err(PathGuardError::NotCanonical),
                ".." => return Err(PathGuardError::ParentComponent),
                name => {
                    check_component(name)?;
                    parts.push(name.to_owned());
                }
            }
        }
        Ok(SafePath(self.assemble(&parts)))
    }

    fn assemble(&self, parts: &[String]) -> PathBuf {
        let mut path = self.root.clone();
        for part in &parts[self.parts.len()..] {
            path.push(part);
        }
        path
    }
}

/// A path a [`PathGuard`] proved to be inside its root.
// Deliberately not `Deserialize`: a path that skipped the guard is not safe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SafePath(PathBuf);

impl SafePath {
    /// The proved path, for the filesystem layer.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Why a path was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathGuardError {
    /// The configured root is not absolute.
    #[error("the configured root is not an absolute path")]
    RootNotAbsolute,
    /// The configured root contains `.` or `..`, so it is not canonical.
    #[error("the configured root contains `.` or `..`")]
    RootNotCanonical,
    /// The candidate is absolute but not under the configured root.
    #[error("the candidate is outside the configured root")]
    OutsideRoot,
    /// A `.` component makes the candidate non-canonical.
    #[error("the candidate contains a `.` component")]
    NotCanonical,
    /// A `..` component walked above the configured root.
    #[error("the candidate escapes the configured root with `..`")]
    EscapesRoot,
    /// A relative entry contains a `..` component, which no entry may.
    #[error("a relative entry must not contain a `..` component")]
    ParentComponent,
    /// A relative entry starts with a separator, so it is not relative.
    #[error("a relative entry must not start with a separator")]
    AbsoluteEntry,
    /// A component is empty: a leading, repeated, or trailing separator.
    #[error("the path contains an empty component")]
    EmptyComponent,
    /// A component carries a drive letter without a root, so it resolves anywhere.
    #[error("the path uses a drive-relative form")]
    DriveForm,
    /// A component names a device on Windows.
    #[error("`{component}` is a reserved device name on Windows")]
    ReservedName {
        /// The rejected component.
        component: String,
    },
    /// A component contains a character a later layer could reinterpret.
    #[error("{character:?} is not allowed in a path component")]
    ForbiddenCharacter {
        /// The rejected character.
        character: char,
    },
    /// A component is longer than a filesystem accepts.
    #[error("a path component is {bytes} bytes, above the {MAX_COMPONENT_BYTES}-byte limit")]
    ComponentTooLong {
        /// The rejected length in bytes.
        bytes: usize,
    },
    /// A component is not valid UTF-8.
    #[error("a path component is not valid UTF-8")]
    InvalidEncoding,
}

fn text_of(value: &OsStr) -> Result<String, PathGuardError> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or(PathGuardError::InvalidEncoding)
}

fn prefixes_agree(candidate: Option<&str>, root: Option<&str>) -> bool {
    match (candidate, root) {
        (Some(candidate), Some(root)) => candidate.eq_ignore_ascii_case(root),
        (None, None) => true,
        _ => false,
    }
}

fn check_component(component: &str) -> Result<(), PathGuardError> {
    if component.is_empty() {
        return Err(PathGuardError::EmptyComponent);
    }
    if component.len() > MAX_COMPONENT_BYTES {
        return Err(PathGuardError::ComponentTooLong {
            bytes: component.len(),
        });
    }
    if let Some(character) = component
        .chars()
        .find(|character| is_forbidden(*character) || is_platform_forbidden(*character))
    {
        return Err(PathGuardError::ForbiddenCharacter { character });
    }
    if is_reserved_name(component) {
        return Err(PathGuardError::ReservedName {
            component: component.to_owned(),
        });
    }
    Ok(())
}

/// Characters no filename may contain on any platform: the NUL terminator, the two path
/// separators, and the confusables that NFKC can fold into one of them.
fn is_forbidden(character: char) -> bool {
    character == '\0'
        || character == '/'
        || character == '\\'
        || CONFUSABLE_SEPARATORS.contains(&character)
}

/// Characters Windows refuses in a filename: the NTFS alternate-data-stream `:` and the
/// six Win32 metacharacters. Every one of them is legal on macOS/APFS and Linux, and real
/// releases are full of them — a live acquire was blocked on
/// `Uchuu Senkan Yamato 2199: Hoshimeguru Hakobune` — so refusing them on every host
/// rejects ordinary media. Sonarr and Radarr sanitize rather than refuse; here the name is
/// refused only where the filesystem would.
#[cfg(windows)]
fn is_platform_forbidden(character: char) -> bool {
    matches!(character, ':' | '<' | '>' | '"' | '|' | '?' | '*')
}

#[cfg(not(windows))]
const fn is_platform_forbidden(_character: char) -> bool {
    false
}

fn is_reserved_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches([' ', '.']);
    RESERVED_NAMES.contains(&stem.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\media\library")
        } else {
            PathBuf::from("/media/library")
        }
    }

    fn parent_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\media")
        } else {
            PathBuf::from("/media")
        }
    }

    fn guard() -> PathGuard {
        PathGuard::new(root()).expect("a canonical absolute root")
    }

    fn parent_guard() -> PathGuard {
        PathGuard::new(parent_root()).expect("a canonical absolute root")
    }

    fn inside(relative: &str) -> PathBuf {
        root().join(relative)
    }

    #[test]
    fn the_root_must_be_absolute() {
        assert_eq!(
            PathGuard::new("media/library"),
            Err(PathGuardError::RootNotAbsolute)
        );
        assert_eq!(PathGuard::new(""), Err(PathGuardError::RootNotAbsolute));
    }

    #[test]
    fn the_root_must_be_canonical() {
        let root = if cfg!(windows) {
            r"C:\media\..\library"
        } else {
            "/media/../library"
        };
        assert_eq!(PathGuard::new(root), Err(PathGuardError::RootNotCanonical));
    }

    #[test]
    fn the_root_is_reported_back() {
        assert_eq!(guard().root(), root());
    }

    #[test]
    fn a_candidate_inside_the_root_is_contained() {
        let resolved = guard()
            .resolve(&inside("a/b.mkv"))
            .expect("inside the root");
        assert_eq!(resolved.as_path(), inside("a/b.mkv"));
    }

    #[test]
    fn a_candidate_equal_to_the_root_is_contained() {
        let guard = guard();
        let resolved = guard.resolve(&root()).expect("the root contains itself");
        assert_eq!(resolved.as_path(), root().as_path());
        assert!(guard.contains(&root()));
    }

    #[test]
    fn a_relative_candidate_is_resolved_against_the_root() {
        let resolved = guard()
            .resolve(Path::new("a/b.mkv"))
            .expect("inside the root");
        assert_eq!(resolved.as_path(), inside("a/b.mkv"));
    }

    #[test]
    fn parent_traversal_above_the_root_is_rejected() {
        assert_eq!(
            guard().resolve(Path::new("../escape.mkv")),
            Err(PathGuardError::EscapesRoot)
        );
        assert_eq!(
            guard().resolve(Path::new("a/../..")),
            Err(PathGuardError::EscapesRoot)
        );
    }

    #[test]
    fn parent_components_that_stay_inside_are_folded() {
        let resolved = guard()
            .resolve(Path::new("a/../b.mkv"))
            .expect("the fold stays inside");
        assert_eq!(resolved.as_path(), inside("b.mkv"));
    }

    #[test]
    fn an_absolute_candidate_outside_the_root_is_rejected() {
        let outside = if cfg!(windows) {
            r"D:\other\file.mkv"
        } else {
            "/etc/passwd"
        };
        assert_eq!(
            guard().resolve(Path::new(outside)),
            Err(PathGuardError::OutsideRoot)
        );
    }

    #[test]
    fn an_absolute_candidate_that_climbs_out_is_rejected() {
        let candidate = if cfg!(windows) {
            r"C:\media\library\..\..\other"
        } else {
            "/media/library/../../other"
        };
        assert_eq!(
            guard().resolve(Path::new(candidate)),
            Err(PathGuardError::OutsideRoot)
        );
    }

    #[test]
    fn a_prefix_match_is_not_containment() {
        let guard = parent_guard();
        let sibling = if cfg!(windows) {
            r"C:\media-evil\file.mkv"
        } else {
            "/media-evil/file.mkv"
        };
        assert!(!guard.contains(Path::new(sibling)));
        assert_eq!(
            guard.resolve(Path::new(sibling)),
            Err(PathGuardError::OutsideRoot)
        );
    }

    #[test]
    fn a_dot_component_is_rejected_because_it_is_not_canonical() {
        assert_eq!(
            guard().resolve(Path::new("./a.mkv")),
            Err(PathGuardError::NotCanonical)
        );
    }

    #[test]
    fn an_empty_candidate_is_rejected() {
        assert_eq!(
            guard().resolve(Path::new("")),
            Err(PathGuardError::EmptyComponent)
        );
    }

    #[test]
    fn separators_that_are_not_a_forward_slash_are_rejected() {
        assert_eq!(
            guard().resolve_relative("a\\b.mkv"),
            Err(PathGuardError::ForbiddenCharacter { character: '\\' })
        );
        assert_eq!(
            guard().resolve_relative("..\\..\\escape.mkv"),
            Err(PathGuardError::ForbiddenCharacter { character: '\\' })
        );
    }

    #[test]
    fn windows_drive_forms_are_rejected() {
        // Windows finds the drive colon first, a Unix host finds the backslash. Both refuse
        // the text, which is the point.
        #[cfg(windows)]
        assert_eq!(
            guard().resolve_relative("C:\\Windows\\System32"),
            Err(PathGuardError::ForbiddenCharacter { character: ':' })
        );
        #[cfg(not(windows))]
        assert_eq!(
            guard().resolve_relative("C:\\Windows\\System32"),
            Err(PathGuardError::ForbiddenCharacter { character: '\\' })
        );
        // `:` is an alternate-data-stream hazard on Windows only. Off Windows it is an
        // ordinary filename character, and refusing it blocked nearly every real release
        // (a live acquire died on `Uchuu Senkan Yamato 2199: Hoshimeguru Hakobune`).
        #[cfg(windows)]
        assert_eq!(
            guard().resolve_relative("C:autoexec.bat"),
            Err(PathGuardError::ForbiddenCharacter { character: ':' })
        );
        #[cfg(not(windows))]
        assert!(
            guard()
                .resolve_relative("Uchuu Senkan Yamato 2199: Hoshimeguru Hakobune.mkv")
                .is_ok(),
            "a colon is legal in a filename on this platform"
        );
        // On Windows this is a prefix plus components; on a Unix host it is one
        // component containing `:` and `\`. Both hosts refuse it, for the backslash.
        assert!(guard().resolve(Path::new(r"C:\Windows\System32")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_relative_candidate_is_refused_rather_than_resolved() {
        assert_eq!(
            guard().resolve(Path::new("C:autoexec.bat")),
            Err(PathGuardError::DriveForm)
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_absolute_candidate_under_the_root_is_contained() {
        let guard = guard();
        assert!(guard.contains(Path::new(r"C:\media\library\a.mkv")));
        assert!(
            guard.contains(Path::new(r"c:\media\library\a.mkv")),
            "drive letters compare case-insensitively"
        );
        assert!(!guard.contains(Path::new(r"D:\media\library\a.mkv")));
    }

    #[test]
    fn windows_reserved_names_are_rejected_in_any_spelling() {
        for name in ["CON", "nul.txt", "LPT1", "com9.mkv", "con ", "CON."] {
            assert_eq!(
                guard().resolve_relative(name),
                Err(PathGuardError::ReservedName {
                    component: name.to_owned(),
                }),
                "`{name}` is a device name"
            );
        }
    }

    #[test]
    fn names_that_only_look_reserved_are_allowed() {
        for name in ["CONS", "console.txt", "com10", "lpt10", "null"] {
            assert!(
                guard().resolve_relative(name).is_ok(),
                "`{name}` is not a device name"
            );
        }
    }

    #[test]
    fn a_reserved_name_is_rejected_wherever_it_appears_in_the_path() {
        assert!(matches!(
            guard().resolve_relative("extras/NUL/report.txt"),
            Err(PathGuardError::ReservedName { .. })
        ));
    }

    #[test]
    fn nul_bytes_are_rejected() {
        assert_eq!(
            guard().resolve_relative("a\0b.mkv"),
            Err(PathGuardError::ForbiddenCharacter { character: '\0' })
        );
    }

    #[test]
    fn confusable_separators_are_rejected() {
        for character in CONFUSABLE_SEPARATORS {
            let name = format!("a{character}b.mkv");
            assert_eq!(
                guard().resolve_relative(&name),
                Err(PathGuardError::ForbiddenCharacter {
                    character: *character,
                }),
                "U+{:04X} could be normalised into a separator",
                u32::from(*character)
            );
        }
    }

    #[test]
    fn a_component_exactly_at_the_limit_is_accepted_and_one_above_is_not() {
        let at_limit = "a".repeat(MAX_COMPONENT_BYTES);
        assert!(guard().resolve_relative(&at_limit).is_ok());
        let over = "a".repeat(MAX_COMPONENT_BYTES + 1);
        assert_eq!(
            guard().resolve_relative(&over),
            Err(PathGuardError::ComponentTooLong {
                bytes: MAX_COMPONENT_BYTES + 1,
            })
        );
    }

    #[test]
    fn an_empty_component_is_rejected_in_every_position() {
        assert_eq!(
            guard().resolve_relative(""),
            Err(PathGuardError::EmptyComponent)
        );
        assert_eq!(
            guard().resolve_relative("a//b.mkv"),
            Err(PathGuardError::EmptyComponent)
        );
        assert_eq!(
            guard().resolve_relative("a/"),
            Err(PathGuardError::EmptyComponent)
        );
    }

    #[test]
    fn a_relative_entry_must_not_be_absolute() {
        assert_eq!(
            guard().resolve_relative("/etc/passwd"),
            Err(PathGuardError::AbsoluteEntry)
        );
    }

    #[test]
    fn a_relative_entry_must_not_contain_a_dot_or_parent_component() {
        assert_eq!(
            guard().resolve_relative("a/./b.mkv"),
            Err(PathGuardError::NotCanonical)
        );
        assert_eq!(
            guard().resolve_relative("a/../b.mkv"),
            Err(PathGuardError::ParentComponent)
        );
        assert_eq!(
            guard().resolve_relative(".."),
            Err(PathGuardError::ParentComponent)
        );
    }

    #[test]
    fn a_relative_entry_resolves_under_the_root() {
        assert_eq!(
            guard()
                .resolve_relative("Season 01/S01E01.mkv")
                .expect("a plain relative entry")
                .as_path(),
            inside("Season 01/S01E01.mkv")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_component_that_is_not_valid_utf8_is_rejected() {
        use std::os::unix::ffi::OsStrExt;
        let name = OsStr::from_bytes(b"a\xffb.mkv");
        assert_eq!(
            guard().resolve(&inside("").join(name)),
            Err(PathGuardError::InvalidEncoding)
        );
    }

    fn safe_component() -> impl Strategy<Value = String> {
        "file-[a-z0-9]{1,8}"
    }

    fn join(parts: &[String]) -> PathBuf {
        let mut path = root();
        for part in parts {
            path.push(part);
        }
        path
    }

    proptest! {
        #[test]
        fn appending_a_relative_component_to_the_root_stays_inside(component in safe_component()) {
            let guard = guard();
            let candidate = root().join(&component);
            prop_assert!(guard.contains(&candidate));
            let resolved = guard.resolve(&candidate).expect("inside the root");
            prop_assert_eq!(resolved.as_path(), candidate.as_path());
        }

        #[test]
        fn a_parent_component_that_escapes_the_root_is_never_contained(
            inner in prop::collection::vec(safe_component(), 0..4),
            extra in 1_usize..4,
        ) {
            let guard = guard();
            let mut candidate = join(&inner);
            for _ in 0..=inner.len() + extra {
                candidate.push("..");
            }
            prop_assert!(!guard.contains(&candidate));
            prop_assert!(guard.resolve(&candidate).is_err());
        }

        #[test]
        fn parent_components_that_stay_inside_fold_back_to_the_root(
            inner in prop::collection::vec(safe_component(), 1..4),
        ) {
            let guard = guard();
            let mut candidate = join(&inner);
            for _ in 0..inner.len() {
                candidate.push("..");
            }
            let resolved = guard.resolve(&candidate).expect("the fold returns to the root");
            let at_root = root();
            prop_assert_eq!(resolved.as_path(), at_root.as_path());
        }

        #[test]
        fn containment_is_reflexive_over_generated_component_lists(
            parts in prop::collection::vec(safe_component(), 0..5),
        ) {
            let guard = guard();
            prop_assert!(guard.contains(&root()));
            prop_assert!(guard.contains(&join(&parts)));
        }

        #[test]
        fn containment_is_transitive_over_generated_component_lists(
            outer in prop::collection::vec(safe_component(), 0..4),
            inner in prop::collection::vec(safe_component(), 0..4),
        ) {
            let guard = guard();
            let candidate = join(&outer);
            prop_assert!(guard.contains(&candidate));
            prop_assert!(guard.contains(&join(&[outer, inner].concat())));
        }

        #[test]
        fn a_sibling_of_the_root_is_never_contained(component in safe_component()) {
            let sibling = parent_root().join(component);
            prop_assert!(!guard().contains(&sibling));
        }

        #[test]
        fn resolve_relative_only_returns_paths_under_the_root(entry in any::<String>()) {
            let guard = guard();
            if let Ok(safe) = guard.resolve_relative(&entry) {
                prop_assert!(safe.as_path().starts_with(root()));
                let again = guard.resolve_relative(&entry).expect("already accepted once");
                prop_assert_eq!(again.as_path(), safe.as_path());
            }
        }

        #[test]
        fn a_relative_entry_with_a_parent_component_is_never_accepted(
            before in prop::collection::vec(safe_component(), 0..3),
            after in prop::collection::vec(safe_component(), 0..3),
        ) {
            let entry = format!("{}/../{}", before.join("/"), after.join("/"));
            prop_assert!(guard().resolve_relative(&entry).is_err());
        }

        #[test]
        fn resolve_relative_never_panics(entry in any::<String>()) {
            let _ = guard().resolve_relative(&entry);
        }

        #[test]
        fn resolve_never_panics(candidate in any::<String>()) {
            let _ = guard().resolve(Path::new(&candidate));
        }
    }
}
