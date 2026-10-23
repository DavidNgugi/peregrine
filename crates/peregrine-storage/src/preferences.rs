//! The singleton playback preference row.
//!
//! One row, read on every playback mint and written only by the desktop's Settings screen. The
//! repository is deliberately tiny: the validation is the domain constructor's, and this module
//! only carries the row across the driver boundary.

use jiff::Timestamp;
use peregrine_domain::PlaybackPreference;
use peregrine_domain::notify::{Consent, ConsentDecision};
use sqlx::{SqliteConnection, SqlitePool};

use crate::error::StorageError;

/// Reads the stored preference, or the default when the table somehow holds no row.
///
/// A missing row is answered with the default rather than an error because the migration inserts
/// it: absence means a database this build did not create, and a player with English defaults is a
/// better answer than a failed playback mint.
pub(crate) async fn read(pool: &SqlitePool) -> Result<PlaybackPreference, StorageError> {
    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT audio_language, subtitle_language, subtitles FROM playback_preference WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    let Some((audio_language, subtitle_language, subtitles)) = row else {
        return Ok(PlaybackPreference::default());
    };
    let subtitles = peregrine_domain::SubtitleMode::parse(&subtitles).ok_or_else(|| {
        StorageError::UnexpectedValue {
            column: "playback_preference.subtitles",
            found: subtitles,
        }
    })?;
    PlaybackPreference::new(&audio_language, &subtitle_language, subtitles).map_err(|error| {
        StorageError::UnexpectedValue {
            column: "playback_preference.language",
            found: error.to_string(),
        }
    })
}

/// Writes the one preference row, replacing the values the previous writer stored.
pub(crate) async fn write(
    connection: &mut SqliteConnection,
    preference: &PlaybackPreference,
    at: Timestamp,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO playback_preference (id, audio_language, subtitle_language, subtitles, updated_at) \
         VALUES (1, ?1, ?2, ?3, ?4) \
         ON CONFLICT (id) DO UPDATE SET \
             audio_language = excluded.audio_language, \
             subtitle_language = excluded.subtitle_language, \
             subtitles = excluded.subtitles, \
             updated_at = excluded.updated_at",
    )
    .bind(preference.audio_language())
    .bind(preference.subtitle_language())
    .bind(preference.subtitles().as_str())
    .bind(at.as_millisecond())
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// The settings key the notification consent is stored under.
///
/// A key of its own in the existing `settings` table rather than a new table: consent is one
/// value, and the settings registry that will own the rest of the notification preferences is
/// PG-E20-01's.
const NOTIFICATION_CONSENT_KEY: &str = "notifications.consent";

/// Reads the user's answer to the notification consent question.
///
/// An absent row is [`Consent::UNASKED`] rather than an error: that is the state every
/// installation starts in, and a user who has never been asked must be asked.
pub(crate) async fn read_notification_consent(pool: &SqlitePool) -> Result<Consent, StorageError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value_json FROM settings WHERE key = ?1")
        .bind(NOTIFICATION_CONSENT_KEY)
        .fetch_optional(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    let Some((value,)) = row else {
        return Ok(Consent::UNASKED);
    };
    parse_consent(&value).ok_or(StorageError::UnexpectedValue {
        column: "settings.notifications.consent",
        found: value,
    })
}

/// Stores the user's answer, replacing whatever was there.
pub(crate) async fn write_notification_consent(
    connection: &mut SqliteConnection,
    consent: Consent,
    at: Timestamp,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json, \
         updated_at = excluded.updated_at",
    )
    .bind(NOTIFICATION_CONSENT_KEY)
    .bind(consent_json(consent))
    .bind(at.as_millisecond())
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// The stored shape of [`Consent`].
///
/// Three fields rather than a bare boolean because "never asked", "declined", and "the OS
/// withdrew what it granted" are different answers, and the policy reads all three.
fn consent_json(consent: Consent) -> String {
    serde_json::json!({
        "decision": match consent.decision {
            ConsentDecision::Unanswered => "unanswered",
            ConsentDecision::Granted => "granted",
            ConsentDecision::Declined => "declined",
        },
        "dontAskAgain": consent.dont_ask_again,
        "osGranted": consent.os_granted,
    })
    .to_string()
}

/// Reads [`consent_json`] back, answering `None` for a row this build cannot use.
fn parse_consent(value: &str) -> Option<Consent> {
    let value: serde_json::Value = serde_json::from_str(value).ok()?;
    let decision = match value.get("decision")?.as_str()? {
        "unanswered" => ConsentDecision::Unanswered,
        "granted" => ConsentDecision::Granted,
        "declined" => ConsentDecision::Declined,
        _ => return None,
    };
    let dont_ask_again = value.get("dontAskAgain")?.as_bool()?;
    let os_granted = match value.get("osGranted") {
        Some(serde_json::Value::Null) | None => None,
        Some(inner) => Some(inner.as_bool()?),
    };
    Some(Consent {
        decision,
        dont_ask_again,
        os_granted,
    })
}

#[cfg(test)]
mod tests {
    use peregrine_domain::{PlaybackPreference, SubtitleMode};
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn the_default_is_read_before_anything_is_written() {
        let directory = TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        assert_eq!(
            database.playback_preference().await.unwrap(),
            PlaybackPreference::default()
        );
    }

    #[tokio::test]
    async fn a_preference_round_trips() {
        let directory = TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        let preference =
            PlaybackPreference::new("fr-ca", "de", SubtitleMode::Forced).expect("valid tags");
        database
            .set_playback_preference(&preference, jiff::Timestamp::UNIX_EPOCH)
            .await
            .unwrap();
        assert_eq!(database.playback_preference().await.unwrap(), preference);
    }

    #[tokio::test]
    async fn an_unasked_consent_is_the_default() {
        let directory = TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        assert_eq!(
            database.notification_consent().await.unwrap(),
            Consent::UNASKED,
            "a user who has never been asked must still be asked"
        );
    }

    #[tokio::test]
    async fn a_consent_round_trips_with_every_field() {
        let directory = TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        for consent in [
            Consent::granted(),
            Consent::declined(true),
            Consent {
                decision: ConsentDecision::Granted,
                dont_ask_again: false,
                os_granted: Some(false),
            },
        ] {
            database
                .set_notification_consent(consent, jiff::Timestamp::UNIX_EPOCH)
                .await
                .unwrap();
            assert_eq!(database.notification_consent().await.unwrap(), consent);
        }
    }

    #[tokio::test]
    async fn a_stored_consent_this_build_cannot_read_is_reported() {
        let directory = TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        crate::test_support::execute(
            &crate::test_support::path_of(&directory),
            "INSERT INTO settings (key, value_json, updated_at) \
             VALUES ('notifications.consent', '{\"decision\":\"maybe\"}', 0)",
        )
        .await;
        assert!(matches!(
            database.notification_consent().await,
            Err(StorageError::UnexpectedValue { .. })
        ));
    }
}
