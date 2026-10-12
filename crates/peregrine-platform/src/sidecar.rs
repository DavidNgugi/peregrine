//! Which components exist, where a sidecar may be found, and what a missing one
//! costs.
//!
//! A sidecar is an executable we hand the user's media to, so nothing here consults
//! `PATH` ([adr/0022](../../../docs/adr/0022-component-packs-and-sidecar-verification.md)
//! §4): a bare name there is whichever build the machine installed first, with flags
//! we did not choose and capabilities we cannot report. Resolution is the three
//! explicit locations of §4 and nothing else, and a file that is found is checked by
//! [`verify`] before it becomes a [`VerifiedSidecar`].
//!
//! The capability half is §6: a component that is absent or refused becomes a named
//! loss — `probe`, `playback`, `transcode`, `subtitle-extract` — so the application
//! starts, states what is unavailable, and never falls back to an unverified binary.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

use crate::sidecar::verify::Artifact;

/// The three third-party executables declared in `sidecars.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// Upstream's GPL build; playback and every media surface.
    Mpv,
    /// The LGPL-only probe; the only source of HDR mastering metadata.
    Ffprobe,
    /// The LGPL-only full build shipped as an on-demand pack; transcode, remux,
    /// and bitmap subtitle extraction.
    Ffmpeg,
}

impl Component {
    /// Every component, in the order a status surface should list them.
    pub const ALL: [Self; 3] = [Self::Mpv, Self::Ffprobe, Self::Ffmpeg];

    /// The name the manifest and the status surface use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Mpv => "mpv",
            Self::Ffprobe => "ffprobe",
            Self::Ffmpeg => "ffmpeg",
        }
    }

    /// The file name on this platform, with the extension Windows requires.
    #[must_use]
    pub fn file_name(self) -> String {
        // Windows appends `.exe` when it executes a name; every other platform
        // executes the bare name. Producing both spellings here rather than at each
        // caller is what stops one candidate path from being spelled differently
        // from another.
        if cfg!(windows) {
            format!("{}.exe", self.name())
        } else {
            self.name().to_owned()
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A user-visible capability, and the components it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Authoritative media inspection: `ffprobe`.
    Probe,
    /// Playback through mpv.
    Playback,
    /// Remux, transcode, and cast delivery: the full `ffmpeg` pack.
    Transcode,
    /// Bitmap subtitle extraction: the full `ffmpeg` pack.
    SubtitleExtract,
}

impl Capability {
    /// Every capability, in the order a status surface should list them.
    pub const ALL: [Self; 4] = [
        Self::Probe,
        Self::Playback,
        Self::Transcode,
        Self::SubtitleExtract,
    ];

    /// The stable name used in Settings and Diagnostics, and in error text.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Playback => "playback",
            Self::Transcode => "transcode",
            Self::SubtitleExtract => "subtitle-extract",
        }
    }

    /// The components this capability cannot work without.
    #[must_use]
    pub fn requires(self) -> &'static [Component] {
        match self {
            Self::Probe => &[Component::Ffprobe],
            Self::Playback => &[Component::Mpv],
            Self::Transcode | Self::SubtitleExtract => &[Component::Ffmpeg],
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Which of the configured locations answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Candidate {
    /// The path the user configured in Settings.
    Override,
    /// The managed component directory the installer and the packs own.
    Managed,
    /// The directory beside the application binary.
    BesideApp,
}

/// Where a sidecar may be found, in the order it is tried.
///
/// The list is the whole policy and it is closed on purpose: no `PATH` candidate
/// exists to select, so no caller can ask for one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// The managed component directory.
    pub managed: PathBuf,
    /// The directory beside the application binary.
    pub beside_app: PathBuf,
}

impl Sources {
    /// The two managed locations, in order.
    ///
    /// An override is absent from this list because it names a file rather than a
    /// directory: [`resolve`] tries it, and only it, before reaching these.
    fn directories(&self) -> [(Candidate, &Path); 2] {
        [
            (Candidate::Managed, self.managed.as_path()),
            (Candidate::BesideApp, self.beside_app.as_path()),
        ]
    }
}

/// Why no location produced a [`VerifiedSidecar`].
#[derive(Debug, Error)]
pub enum ResolveError {
    /// The configured override is empty or only whitespace, so it is a broken
    /// setting rather than an instruction not to override.
    #[error("the sidecar override for `{component}` is empty")]
    EmptyOverride {
        /// The component that was being resolved.
        component: Component,
    },
    /// The configured override is not valid UTF-8, so it cannot be reported or
    /// recorded as a path the user can act on.
    #[error("the sidecar override for `{component}` is not valid UTF-8")]
    InvalidOverride {
        /// The component that was being resolved.
        component: Component,
    },
    /// No location held a file that passed verification.
    #[error("no verified `{component}` sidecar was found in {tried} location(s); last: {source}")]
    NotResolved {
        /// The component that was being resolved.
        component: Component,
        /// How many locations were tried.
        tried: usize,
        /// The failure at the last location tried.
        source: Box<VerifyError>,
    },
}

/// Resolves one component to the first location that verifies.
///
/// An explicit override is final: when it is set and does not verify, this fails
/// instead of continuing to the managed copy. Falling through would run a different
/// binary from the one the user chose and hide the mistake, which is the failure
/// mode the explicit order exists to prevent.
pub fn resolve(
    component: Component,
    override_path: Option<&Path>,
    sources: &Sources,
    artifact: &Artifact,
) -> Result<VerifiedSidecar, ResolveError> {
    if let Some(path) = override_path {
        // Whitespace is a typed mistake, not an absence: a setting holding spaces
        // must not silently mean "no override" (AGENTS §4 Empty).
        if path.as_os_str().is_empty() || path.to_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(ResolveError::EmptyOverride { component });
        }
        // A lossy comparison would verify one path and record another.
        if path.to_str().is_none() {
            return Err(ResolveError::InvalidOverride { component });
        }
        // A user who points at a specific file gets that file or a refusal. The
        // managed locations are not consulted, because answering with a different
        // binary than the one named is worse than answering with nothing.
        //
        // The override names the file itself, so no component name is appended: a
        // user pointing at `/opt/ffmpeg-nightly/bin/ffmpeg` names a binary, not a
        // directory to search.
        return verify_candidate(component, Candidate::Override, path, artifact).map_err(
            |source| ResolveError::NotResolved {
                component,
                tried: 1,
                source: Box::new(source),
            },
        );
    }

    let mut tried = 0_usize;
    let mut last = None;
    for (candidate, directory) in sources.directories() {
        tried += 1;
        match verify_candidate(
            component,
            candidate,
            &directory.join(component.file_name()),
            artifact,
        ) {
            Ok(verified) => return Ok(verified),
            Err(error) => last = Some(error),
        }
    }

    // `Sources` always yields the managed and beside-app candidates, so the loop
    // runs at least twice and `last` is always set. Both arms still return a typed
    // refusal rather than a `None` a caller could ignore.
    let source = last.unwrap_or_else(|| VerifyError::Missing {
        path: sources.managed.join(component.file_name()),
        source: io::Error::new(io::ErrorKind::NotFound, "no candidate location"),
    });
    Err(ResolveError::NotResolved {
        component,
        tried,
        source: Box::new(source),
    })
}

/// Verifies one candidate, logging the refusal that is worth a log line.
///
/// An absent file is the ordinary state before a component is installed; a file
/// that is present and refused is a tampered or corrupted installation, which is
/// the case the log exists for.
fn verify_candidate(
    component: Component,
    candidate: Candidate,
    path: &Path,
    artifact: &Artifact,
) -> Result<VerifiedSidecar, VerifyError> {
    match VerifiedSidecar::verify(path, artifact) {
        Ok(verified) => Ok(verified),
        Err(error) => {
            if !matches!(error, VerifyError::Missing { .. }) {
                warn!(
                    component = component.name(),
                    candidate = ?candidate,
                    path = %path.display(),
                    error = %error,
                    "refused a sidecar that failed verification"
                );
            }
            Err(error)
        }
    }
}

/// A capability that is available, with the token that permits its use.
///
/// The token is borrowed from the caller's [`VerifiedSidecar`] rather than copied,
/// because the token is deliberately not `Clone`: a second token that outlives the
/// verification behind it is exactly what the capability is meant to prevent.
#[derive(Debug)]
pub struct Granted<'a> {
    /// What became available.
    pub capability: Capability,
    /// The verified component that provides it.
    pub sidecar: &'a VerifiedSidecar,
}

/// A capability that is not available, and which components explain it.
#[derive(Debug)]
pub struct Loss {
    /// What is unavailable.
    pub capability: Capability,
    /// One entry per blocked component, each naming why.
    pub reasons: Vec<(Component, String)>,
}

impl Loss {
    /// The blocked components' names, in [`Capability::requires`] order.
    #[must_use]
    pub fn components(&self) -> Vec<&'static str> {
        self.reasons
            .iter()
            .map(|(component, _)| component.name())
            .collect()
    }
}

/// What the application can do, and what it cannot, from the components it found.
#[derive(Debug)]
pub struct CapabilityReport<'a> {
    /// Capabilities whose components all verified.
    pub granted: Vec<Granted<'a>>,
    /// Capabilities missing at least one component.
    pub lost: Vec<Loss>,
}

impl<'a> CapabilityReport<'a> {
    /// Maps each component's outcome onto the capabilities that need it.
    ///
    /// The application starts with this: a capability is available only when every
    /// component it requires verified, and a missing or refused component is a named
    /// loss instead of a fallback to an unverified binary or a failure halfway
    /// through a job ([adr/0022](../../../docs/adr/0022-component-packs-and-sidecar-verification.md)
    /// §6).
    ///
    /// A component that serves more than one capability lends its token to each; a
    /// component that was never offered at all is reported as `not installed` rather
    /// than dropped from the list.
    #[must_use]
    pub fn from_outcomes(
        outcomes: &'a [(Component, Result<VerifiedSidecar, VerifyError>)],
    ) -> Self {
        let mut granted = Vec::new();
        let mut lost = Vec::new();

        for capability in Capability::ALL {
            let mut reasons = Vec::new();
            let mut tokens = Vec::new();
            for component in capability.requires() {
                match outcomes
                    .iter()
                    .find(|(candidate, _)| candidate == component)
                {
                    Some((_, Ok(sidecar))) => tokens.push(sidecar),
                    Some((_, Err(error))) => reasons.push((*component, error.to_string())),
                    None => reasons.push((*component, "not installed".to_owned())),
                }
            }

            if reasons.is_empty() {
                for sidecar in tokens {
                    granted.push(Granted {
                        capability,
                        sidecar,
                    });
                }
            } else {
                lost.push(Loss {
                    capability,
                    reasons,
                });
            }
        }

        Self { granted, lost }
    }

    /// The loss for `capability`, when it is unavailable.
    #[must_use]
    pub fn loss(&self, capability: Capability) -> Option<&Loss> {
        self.lost.iter().find(|loss| loss.capability == capability)
    }
}

pub mod verify;

// Re-exported here and at the crate root so a spawn path never has to reach through
// a submodule to name the token it must hold; `SafePath` sits at the root of
// `peregrine-domain` for the same reason.
pub use verify::{InstallError, InstallRefusal, VerifiedSidecar, VerifyError, install};

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
