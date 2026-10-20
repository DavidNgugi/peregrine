//! The profile commands: the household accounts the desktop and every paired browser share.
//!
//! Four thin commands over the runtime's account reads and writes. The profile is the account
//! the runtime scopes every personal read by, so selecting one here changes the watchlist,
//! progress, continue-watching, and notification reads of the whole installation.

use peregrine_app::AccountRow;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// One profile the picker draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    /// The account id every scoped row carries.
    #[specta(type = Number)]
    pub id: i64,
    /// The display name.
    pub name: String,
    /// `adult`, `child`, or `guest`.
    pub kind: String,
    /// `initials` or `custom`.
    pub avatar_kind: String,
    /// A `DiceBear` data URL or a 128 px upload.
    pub avatar_ref: Option<String>,
    /// Whether this profile's scope hides adult and unrated content.
    pub restricted: bool,
    /// Whether the profile may start a download.
    pub can_acquire: bool,
    /// Whether this is the profile the installation is watching as.
    pub active: bool,
}

impl ProfileDto {
    fn of(account: &AccountRow, active: Option<i64>) -> Self {
        Self {
            id: account.id,
            name: account.name.clone(),
            kind: account.kind.clone(),
            avatar_kind: account.avatar_kind.clone(),
            avatar_ref: account.avatar_ref.clone(),
            restricted: account.is_restricted(),
            can_acquire: account.can_acquire,
            active: active == Some(account.id),
        }
    }
}

/// The profile a command names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRef {
    /// The account row.
    #[specta(type = Number)]
    pub id: i64,
}

/// Every profile, with the active one marked.
#[tauri::command]
#[specta::specta]
pub async fn list_profiles(state: State<'_, RuntimeState>) -> Result<Vec<ProfileDto>, AppError> {
    let runtime = state.runtime()?;
    let active = runtime.active_profile().await?.map(|profile| profile.id);
    let profiles = runtime.profiles().await?;
    Ok(guard_response(
        "profiles::list_profiles",
        profiles
            .iter()
            .map(|profile| ProfileDto::of(profile, active))
            .collect(),
    ))
}

/// Creates a profile and makes it active.
#[tauri::command]
#[specta::specta]
pub async fn create_profile(
    state: State<'_, RuntimeState>,
    name: String,
    kind: String,
    avatar_ref: Option<String>,
) -> Result<ProfileDto, AppError> {
    let runtime = state.runtime()?;
    let account = runtime
        .create_profile(&name, &kind, avatar_ref.as_deref())
        .await?;
    Ok(guard_response(
        "profiles::create_profile",
        ProfileDto::of(&account, Some(account.id)),
    ))
}

/// Makes one profile active.
#[tauri::command]
#[specta::specta]
pub async fn select_profile(
    state: State<'_, RuntimeState>,
    profile: ProfileRef,
) -> Result<ProfileDto, AppError> {
    let runtime = state.runtime()?;
    let account = runtime.select_profile(profile.id).await?;
    Ok(guard_response(
        "profiles::select_profile",
        ProfileDto::of(&account, Some(account.id)),
    ))
}

/// Writes one profile's name and avatar.
#[tauri::command]
#[specta::specta]
pub async fn update_profile(
    state: State<'_, RuntimeState>,
    profile: ProfileRef,
    name: String,
    avatar_ref: Option<String>,
) -> Result<ProfileDto, AppError> {
    let runtime = state.runtime()?;
    let account = runtime
        .update_profile(profile.id, &name, avatar_ref.as_deref())
        .await?;
    let active = runtime.active_profile().await?.map(|profile| profile.id);
    Ok(guard_response(
        "profiles::update_profile",
        ProfileDto::of(&account, active),
    ))
}
