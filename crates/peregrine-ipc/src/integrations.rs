//! The `integrations` and `markers` namespaces.
//!
//! Eight thin commands: what this installation has, four choices about it, three credential
//! actions, and the marker reads a player needs. **No command here touches a keyring.** A probe, a
//! save, and a removal are jobs (`ARCHITECTURE` §7 rule 2); the choices are database writes; and
//! the marker reads answer the cache.
//!
//! **A secret is never returned.** The DTO carries whether a field is present, never its value,
//! length, or prefix. A candidate value travels in the request once and is moved into the job's
//! parked map, where the journal cannot see it (SECURITY.md T13).

use peregrine_app::integrations::{CredentialField, IntegrationRegistry, IntegrationStatus};
use peregrine_app::jobs::integration_credential::Action;
use peregrine_app::{
    IntegrationCapability, IntegrationCategory, IntegrationOutcome, PlaybackMarker, Secret,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::providers::CredentialStoreDto;
use crate::runtime::RuntimeState;

/// Where an integration's card sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationCategoryDto {
    /// Titles, seasons, episodes, and artwork.
    Metadata,
    /// Intro, recap, credits, and preview windows.
    SkipMarkers,
    /// Subtitle search and download.
    Subtitles,
    /// Watch-history scrobbling.
    Tracking,
    /// Indexers and download sources.
    Acquisition,
}

impl From<IntegrationCategory> for IntegrationCategoryDto {
    fn from(category: IntegrationCategory) -> Self {
        match category {
            IntegrationCategory::Metadata => Self::Metadata,
            IntegrationCategory::SkipMarkers => Self::SkipMarkers,
            IntegrationCategory::Subtitles => Self::Subtitles,
            IntegrationCategory::Tracking => Self::Tracking,
            IntegrationCategory::Acquisition => Self::Acquisition,
        }
    }
}

/// One thing an integration can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationCapabilityDto {
    /// Search and enrich catalogue rows.
    Metadata,
    /// Read skip markers.
    Markers,
    /// Search and download subtitles.
    Subtitles,
    /// Report playback progress.
    Scrobble,
    /// Search releases or talk to a download source.
    Acquisition,
}

impl From<IntegrationCapability> for IntegrationCapabilityDto {
    fn from(capability: IntegrationCapability) -> Self {
        match capability {
            IntegrationCapability::Metadata => Self::Metadata,
            IntegrationCapability::Markers => Self::Markers,
            IntegrationCapability::Subtitles => Self::Subtitles,
            IntegrationCapability::Scrobble => Self::Scrobble,
            IntegrationCapability::Acquisition => Self::Acquisition,
        }
    }
}

/// What the last bounded probe answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationOutcomeDto {
    /// The service answered.
    Working,
    /// The service refused the credential.
    Unauthorized,
    /// The service asked to be left alone.
    RateLimited,
    /// The service could not be reached or answered unusably.
    Unreachable,
    /// The OS refused to read the keyring.
    KeychainDenied,
    /// No credential is stored and one is required.
    NotConfigured,
}

impl From<IntegrationOutcome> for IntegrationOutcomeDto {
    fn from(outcome: IntegrationOutcome) -> Self {
        match outcome {
            IntegrationOutcome::Working => Self::Working,
            IntegrationOutcome::Unauthorized => Self::Unauthorized,
            IntegrationOutcome::RateLimited => Self::RateLimited,
            IntegrationOutcome::Unreachable => Self::Unreachable,
            IntegrationOutcome::KeychainDenied => Self::KeychainDenied,
            IntegrationOutcome::NotConfigured => Self::NotConfigured,
        }
    }
}

/// One credential a person may enter.
///
/// `present` is the only thing the UI learns about a stored value: the field is drawn as
/// "Replace" when it is true and as empty when it is false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CredentialFieldDto {
    /// The stable name the save command takes.
    pub name: String,
    /// What the screen calls it.
    pub label: String,
    /// Whether the field is a password input.
    pub secret: bool,
    /// Whether the integration works without it.
    pub required: bool,
    /// Whether a value is already stored.
    pub present: bool,
}

impl CredentialFieldDto {
    /// The DTO for one field.
    fn of(field: &CredentialField, present: bool) -> Self {
        Self {
            name: field.name.to_owned(),
            label: field.label.to_owned(),
            secret: field.secret,
            required: field.required,
            present,
        }
    }
}

/// One integration, as the settings screen and the read-only TV status draw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // A wire DTO: enabled/configured/submission are three independent facts.
pub struct IntegrationDto {
    /// The stable id the commands take.
    pub id: String,
    /// What to call it.
    pub display_name: String,
    /// One sentence about it.
    pub description: String,
    /// Where its documentation lives.
    pub docs: String,
    /// Where its card sits.
    pub category: IntegrationCategoryDto,
    /// What it can be asked for.
    pub capabilities: Vec<IntegrationCapabilityDto>,
    /// Whether a credential is required.
    pub requires_key: bool,
    /// Whether it accepts a key but works without one.
    pub key_optional: bool,
    /// Whether it accepts contributed timestamps.
    pub submission_support: bool,
    /// Whether it is available without being enabled.
    pub always_on: bool,
    /// Whether it may be asked.
    pub enabled: bool,
    /// Its order within its category; lower runs first.
    pub priority: u32,
    /// Whether contributions are allowed.
    pub submission_enabled: bool,
    /// Whether every required field is present.
    pub configured: bool,
    /// Whether a person has to fix something before this integration can work.
    ///
    /// Core's answer rather than a screen's: a denied keychain, a refused credential, a rate
    /// limit, and a missing required value are the same question to every client.
    pub needs_attention: bool,
    /// What the last credential read answered.
    pub store: CredentialStoreDto,
    /// What the last probe answered, when one has run.
    pub last_outcome: Option<IntegrationOutcomeDto>,
    /// When the last probe ran, in Unix milliseconds.
    #[specta(type = Option<Number>)]
    pub last_test_at: Option<i64>,
    /// The stable code the last failed probe recorded.
    pub last_error_code: Option<String>,
    /// The fields a person may enter.
    pub fields: Vec<CredentialFieldDto>,
}

impl IntegrationDto {
    /// The DTO for one merged status.
    fn of(status: &IntegrationStatus, registry: &IntegrationRegistry) -> Self {
        let manifest = status.manifest;
        Self {
            id: manifest.id.to_owned(),
            display_name: manifest.display_name.to_owned(),
            description: manifest.description.to_owned(),
            docs: manifest.docs.to_owned(),
            category: manifest.category.into(),
            capabilities: manifest
                .capabilities
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            requires_key: manifest.requires_key(),
            key_optional: manifest.key_optional(),
            submission_support: manifest.submission_support,
            always_on: manifest.always_on,
            enabled: status.preference.enabled,
            priority: status.preference.priority,
            submission_enabled: status.preference.submission_enabled,
            configured: status.configured,
            needs_attention: status
                .last_outcome
                .is_some_and(IntegrationOutcome::needs_attention)
                || (manifest.requires_key() && !status.configured),
            store: status.store.into(),
            last_outcome: status.last_outcome.map(Into::into),
            last_test_at: status.last_test_at,
            last_error_code: status.last_error_code.clone(),
            fields: manifest
                .credential_fields
                .iter()
                .map(|field| CredentialFieldDto::of(field, registry.field_present(manifest, field)))
                .collect(),
        }
    }
}

/// Every integration this build knows, with this installation's choices.
#[tauri::command]
#[specta::specta]
pub async fn list_integrations(
    state: State<'_, RuntimeState>,
) -> Result<Vec<IntegrationDto>, AppError> {
    let runtime = state.runtime()?;
    let registry = runtime.integration_registry();
    let rows = runtime
        .integration_status()
        .iter()
        .map(|status| IntegrationDto::of(status, &registry))
        .collect();
    Ok(guard_response("integrations::list_integrations", rows))
}

/// Enables or disables one integration.
#[tauri::command]
#[specta::specta]
pub async fn set_integration_enabled(
    state: State<'_, RuntimeState>,
    id: String,
    enabled: bool,
) -> Result<(), AppError> {
    state
        .runtime()?
        .set_integration_preference(&id, Some(enabled), None, None)
        .await?;
    Ok(())
}

/// Sets one integration's order within its category.
#[tauri::command]
#[specta::specta]
pub async fn set_integration_priority(
    state: State<'_, RuntimeState>,
    id: String,
    priority: u32,
) -> Result<(), AppError> {
    state
        .runtime()?
        .set_integration_preference(&id, None, Some(priority), None)
        .await?;
    Ok(())
}

/// Consents to, or withdraws consent from, one integration's submissions.
#[tauri::command]
#[specta::specta]
pub async fn set_integration_submission(
    state: State<'_, RuntimeState>,
    id: String,
    enabled: bool,
) -> Result<(), AppError> {
    state
        .runtime()?
        .set_integration_preference(&id, None, None, Some(enabled))
        .await?;
    Ok(())
}

/// Starts a bounded connection test for one integration.
#[tauri::command]
#[specta::specta]
pub async fn test_integration(
    state: State<'_, RuntimeState>,
    id: String,
) -> Result<JobStarted, AppError> {
    let started = state
        .runtime()?
        .start_integration_credential(&id, Action::Verify, Vec::new())
        .await?;
    Ok(guard_response(
        "integrations::test_integration",
        JobStarted::from(started),
    ))
}

/// One candidate value, moved into the job and never logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CredentialValueDto {
    /// The field name, as the manifest spells it.
    pub field: String,
    /// The candidate value.
    pub value: String,
}

/// Proves and stores one integration's credentials, or clears them when the list is empty.
#[tauri::command]
#[specta::specta]
pub async fn set_integration_credentials(
    state: State<'_, RuntimeState>,
    integration: String,
    values: Vec<CredentialValueDto>,
) -> Result<JobStarted, AppError> {
    let runtime = state.runtime()?;
    let Some(manifest) = peregrine_app::find_integration(&integration) else {
        return Err(invalid("That integration is not one this build knows."));
    };
    let candidates = credential_candidates(manifest, values)?;
    let started = runtime
        .start_integration_credential(&integration, Action::Replace, candidates)
        .await?;
    Ok(guard_response(
        "integrations::set_integration_credentials",
        JobStarted::from(started),
    ))
}

/// The one refusal shape every credential parse uses.
fn invalid(detail: &'static str) -> AppError {
    AppError::new(ErrorCode::InvalidInput).with_detail(detail)
}

/// Turns the wire's candidate list into the values the job may store.
///
/// Parsing rather than deciding: the manifest says which fields exist and which are required,
/// an empty optional field is omitted, and an empty required one is refused before a job exists.
fn credential_candidates(
    manifest: &peregrine_app::IntegrationManifest,
    values: Vec<CredentialValueDto>,
) -> Result<Vec<(String, Secret)>, AppError> {
    let mut candidates = Vec::with_capacity(values.len());
    for entry in values {
        let Some(field) = manifest
            .credential_fields
            .iter()
            .find(|field| field.name == entry.field)
        else {
            return Err(invalid(
                "That credential field does not belong to this integration.",
            ));
        };
        let value = entry.value.trim();
        if value.is_empty() {
            if field.required {
                return Err(invalid("Fill in the required credential before saving."));
            }
            continue;
        }
        candidates.push((field.name.to_owned(), Secret::new(value)));
    }
    if candidates.is_empty() {
        return Err(invalid("Enter at least one credential before saving."));
    }
    Ok(candidates)
}

/// Removes every stored value for one integration.
#[tauri::command]
#[specta::specta]
pub async fn clear_integration_credentials(
    state: State<'_, RuntimeState>,
    id: String,
) -> Result<JobStarted, AppError> {
    let started = state
        .runtime()?
        .start_integration_credential(&id, Action::Forget, Vec::new())
        .await?;
    Ok(guard_response(
        "integrations::clear_integration_credentials",
        JobStarted::from(started),
    ))
}

/// One marker window, as the player draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MarkerDto {
    /// What the window is.
    pub kind: MarkerKindDto,
    /// Where it opens, in milliseconds.
    #[specta(type = Number)]
    pub start_ms: i64,
    /// Where it closes, or `null` for credits that run to the end.
    #[specta(type = Option<Number>)]
    pub end_ms: Option<i64>,
    /// Which source said so.
    pub source: MarkerSourceDto,
    /// How much the source is trusted, in whole percent.
    pub confidence: u8,
}

/// The four marker kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum MarkerKindDto {
    /// The opening sequence.
    Intro,
    /// A previously-on sequence.
    Recap,
    /// The end credits.
    Credits,
    /// A next-episode preview.
    Preview,
}

/// The sources a marker can come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum MarkerSourceDto {
    /// The file's own chapters.
    Chapters,
    /// `TheIntroDB`.
    IntroDb,
    /// `AniSkip`.
    AniSkip,
    /// `IntroHater`.
    IntroHater,
}

impl From<PlaybackMarker> for MarkerDto {
    fn from(marker: PlaybackMarker) -> Self {
        Self {
            kind: match marker.kind() {
                peregrine_app::MarkerKind::Intro => MarkerKindDto::Intro,
                peregrine_app::MarkerKind::Recap => MarkerKindDto::Recap,
                peregrine_app::MarkerKind::Credits => MarkerKindDto::Credits,
                peregrine_app::MarkerKind::Preview => MarkerKindDto::Preview,
            },
            start_ms: marker.start_ms(),
            end_ms: marker.end_ms(),
            source: match marker.source() {
                peregrine_app::MarkerSource::Chapters => MarkerSourceDto::Chapters,
                peregrine_app::MarkerSource::IntroDb => MarkerSourceDto::IntroDb,
                peregrine_app::MarkerSource::AniSkip => MarkerSourceDto::AniSkip,
                peregrine_app::MarkerSource::IntroHater => MarkerSourceDto::IntroHater,
            },
            confidence: marker.confidence().percent(),
        }
    }
}

/// The subject a marker read or refresh is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MarkerSubjectDto {
    /// The title.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The episode, or `null` for a film.
    #[specta(type = Option<Number>)]
    pub episode_id: Option<i64>,
    /// The file's runtime, when the player knows it.
    #[specta(type = Option<Number>)]
    pub duration_ms: Option<i64>,
}

/// The markers the cache holds for a playback subject.
#[tauri::command]
#[specta::specta]
pub async fn markers_for_playback(
    state: State<'_, RuntimeState>,
    subject: MarkerSubjectDto,
) -> Result<Vec<MarkerDto>, AppError> {
    let markers = state
        .runtime()?
        .markers_for_playback(subject.title_id, subject.episode_id, subject.duration_ms)
        .await?;
    Ok(guard_response(
        "integrations::markers_for_playback",
        markers.into_iter().map(MarkerDto::from).collect(),
    ))
}

/// Starts a background refresh of the marker cache for a playback subject.
#[tauri::command]
#[specta::specta]
pub async fn refresh_playback_markers(
    state: State<'_, RuntimeState>,
    subject: MarkerSubjectDto,
) -> Result<(), AppError> {
    let runtime = state.runtime_arc()?;
    runtime.start_marker_refresh(subject.title_id, subject.episode_id, subject.duration_ms);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(field: &str, value: &str) -> CredentialValueDto {
        CredentialValueDto {
            field: field.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn an_optional_empty_field_is_omitted_and_a_required_one_is_refused() {
        let intro = peregrine_app::find_integration("theintrodb").unwrap();
        assert!(
            credential_candidates(intro, vec![value("apiKey", "   ")]).is_err(),
            "nothing to store is not a save"
        );

        let tmdb = peregrine_app::find_integration("tmdb").unwrap();
        assert!(credential_candidates(tmdb, vec![value("apiKey", "")]).is_err());
    }

    #[test]
    fn a_candidate_is_trimmed_and_counts_once() {
        let tmdb = peregrine_app::find_integration("tmdb").unwrap();
        let values = credential_candidates(
            tmdb,
            vec![value("apiKey", "  a-key\n"), value("apiKey", "second")],
        )
        .expect("a non-empty required value");

        assert_eq!(values.len(), 2);
        assert_eq!(values[0].1.expose_secret(), "a-key");
    }

    #[test]
    fn a_field_that_does_not_belong_is_refused() {
        let tmdb = peregrine_app::find_integration("tmdb").unwrap();
        assert!(
            credential_candidates(tmdb, vec![value("username", "someone")]).is_err(),
            "a manifest field list is the contract, not a suggestion"
        );
    }
}
