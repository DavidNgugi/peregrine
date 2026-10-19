//! The playback preference commands: the languages a player chooses tracks by.
//!
//! Two thin commands. The domain constructor validates the tags, the runtime reads and writes
//! the singleton row, and this module only crosses the DTO boundary.

use peregrine_app::{PlaybackPreference, SubtitleMode};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// How the player picks a subtitle track, as the settings screen selects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleModeDto {
    /// Match the preferred language, else the row's own default.
    Auto,
    /// Start with no subtitle track.
    Off,
    /// Show only the forced narrative track.
    Forced,
}

impl From<SubtitleMode> for SubtitleModeDto {
    fn from(mode: SubtitleMode) -> Self {
        match mode {
            SubtitleMode::Auto => Self::Auto,
            SubtitleMode::Off => Self::Off,
            SubtitleMode::Forced => Self::Forced,
        }
    }
}

impl From<SubtitleModeDto> for SubtitleMode {
    fn from(mode: SubtitleModeDto) -> Self {
        match mode {
            SubtitleModeDto::Auto => Self::Auto,
            SubtitleModeDto::Off => Self::Off,
            SubtitleModeDto::Forced => Self::Forced,
        }
    }
}

/// The account's playback language preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackPreferencesDto {
    /// The preferred audio language, as a BCP-47-ish tag.
    pub audio_language: String,
    /// The preferred subtitle language, as a BCP-47-ish tag.
    pub subtitle_language: String,
    /// How the player picks a subtitle track.
    pub subtitles: SubtitleModeDto,
}

impl PlaybackPreferencesDto {
    /// The DTO for one stored preference.
    fn of(preference: &PlaybackPreference) -> Self {
        Self {
            audio_language: preference.audio_language().to_owned(),
            subtitle_language: preference.subtitle_language().to_owned(),
            subtitles: preference.subtitles().into(),
        }
    }

    /// The validated preference this DTO names.
    fn into_preference(self) -> Result<PlaybackPreference, AppError> {
        PlaybackPreference::new(
            &self.audio_language,
            &self.subtitle_language,
            self.subtitles.into(),
        )
        .map_err(|error| AppError::new(ErrorCode::InvalidInput).with_detail(error.to_string()))
    }
}

/// The languages a player chooses audio and subtitle tracks by.
#[tauri::command]
#[specta::specta]
pub async fn playback_preferences(
    state: State<'_, RuntimeState>,
) -> Result<PlaybackPreferencesDto, AppError> {
    let runtime = state.runtime()?;
    let preference = runtime.playback_preference().await?;
    Ok(guard_response(
        "playback::playback_preferences",
        PlaybackPreferencesDto::of(&preference),
    ))
}

/// Replaces the playback language preference and answers the stored value.
#[tauri::command]
#[specta::specta]
pub async fn set_playback_preferences(
    state: State<'_, RuntimeState>,
    preferences: PlaybackPreferencesDto,
) -> Result<PlaybackPreferencesDto, AppError> {
    let runtime = state.runtime()?;
    let preference = preferences.into_preference()?;
    runtime.set_playback_preference(&preference).await?;
    Ok(guard_response(
        "playback::set_playback_preferences",
        PlaybackPreferencesDto::of(&preference),
    ))
}
