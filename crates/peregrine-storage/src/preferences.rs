//! The singleton playback preference row.
//!
//! One row, read on every playback mint and written only by the desktop's Settings screen. The
//! repository is deliberately tiny: the validation is the domain constructor's, and this module
//! only carries the row across the driver boundary.

use jiff::Timestamp;
use peregrine_domain::PlaybackPreference;
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

#[cfg(test)]
mod tests {
    use peregrine_domain::{PlaybackPreference, SubtitleMode};
    use tempfile::TempDir;

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
}
