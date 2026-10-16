//! The quality profile a release is chosen against.
//!
//! The row is three scalars plus a JSON document, so a profile is assembled here rather than read
//! as one blob: rules_json is only the rules, and the name, cutoff, and upgrade flag are columns
//! the profile type also wants. Parsing through the domain's own Deserialize is what keeps a
//! hand-edited file from widening a preference order the constructor would have refused.

use peregrine_domain::{
    AudioCodec, DEFAULT_HD_CEILING_BYTES, GroupRules, QualityProfile, QualityRank, QualityRules,
    ReleaseSource, Resolution, SizeCaps, VideoCodec,
};
use sqlx::{SqliteConnection, SqlitePool};

use crate::error::StorageError;

/// The rules the built-in default profile starts with.
///
/// A preference order, best first: UHD and disc-grade sources outrank the HD web baseline, so the
/// best release available wins. Pre-release and cam sources (cam, ts, tc, r5) are left out of the
/// accept list entirely, because a list admits exactly what it names. Each value is spelled once —
/// the constructor refuses a duplicate.
pub(crate) fn default_rules() -> QualityRules {
    QualityRules::new(
        vec![
            Resolution::UltraHd,
            Resolution::QuadHd,
            Resolution::FullHd,
            Resolution::Hd,
            Resolution::Sd576,
            Resolution::Sd480,
        ],
        vec![
            ReleaseSource::Remux,
            ReleaseSource::BluRay,
            ReleaseSource::BdRip,
            ReleaseSource::BrRip,
            ReleaseSource::WebDl,
            ReleaseSource::WebRip,
            ReleaseSource::Hdtv,
            ReleaseSource::HdRip,
            ReleaseSource::DvdRip,
        ],
        vec![
            VideoCodec::H265,
            VideoCodec::Av1,
            VideoCodec::H264,
            VideoCodec::Vp9,
        ],
        vec![
            AudioCodec::TrueHd,
            AudioCodec::DtsHd,
            AudioCodec::DtsX,
            AudioCodec::Atmos,
            AudioCodec::Dts,
            AudioCodec::Flac,
            AudioCodec::Eac3,
            AudioCodec::Ac3,
            AudioCodec::Opus,
            AudioCodec::Aac,
            AudioCodec::Mp3,
            AudioCodec::Pcm,
        ],
        SizeCaps::new(None, Some(DEFAULT_HD_CEILING_BYTES))
            .expect("an unset floor cannot exceed any ceiling"),
        GroupRules::new(Vec::new(), Vec::new()).expect("no group is listed twice"),
    )
    .expect("the built-in lists spell each value once")
}

/// Inserts the built-in default profile when `quality_profile` has no row at all.
///
/// `WHERE NOT EXISTS` makes it idempotent: a retry, or a second process, finds a row already there
/// and writes nothing. Answers whether this call inserted it.
pub(crate) async fn insert_default(
    connection: &mut SqliteConnection,
    name: &str,
    rules_json: &str,
    cutoff_rank: i64,
    upgrade_allowed: bool,
    at: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO quality_profile (name, is_default, rules_json, cutoff_rank, upgrade_allowed, updated_at) \
         SELECT ?1, 1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (SELECT 1 FROM quality_profile)",
    )
    .bind(name)
    .bind(rules_json)
    .bind(cutoff_rank)
    .bind(if upgrade_allowed { 1 } else { 0 })
    .bind(at)
    .execute(connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The one profile marked is_default, or None when the installation has not chosen one.
///
/// # Errors
///
/// StorageError::InvalidJsonField when rules_json is not the rules this build writes, and
/// StorageError::UnexpectedValue when a scalar is outside its domain or more than one row claims
/// to be the default.
pub(crate) async fn default_profile(
    pool: &SqlitePool,
) -> Result<Option<QualityProfile>, StorageError> {
    let rows = sqlx::query_as::<_, ProfileRow>(
        "SELECT name, rules_json, cutoff_rank, upgrade_allowed FROM quality_profile WHERE is_default = 1",
    )
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    match rows.as_slice() {
        [] => Ok(None),
        [row] => row.to_profile().map(Some),
        // More than one default is a schema the write path should never have produced; failing
        // closed is what keeps "the" profile from depending on row order.
        _ => Err(StorageError::UnexpectedValue {
            column: "is_default",
            found: "more than one quality profile is marked default".to_owned(),
        }),
    }
}

/// The columns a profile is assembled from.
#[derive(sqlx::FromRow)]
struct ProfileRow {
    name: String,
    rules_json: String,
    cutoff_rank: i64,
    upgrade_allowed: i64,
}

impl ProfileRow {
    fn to_profile(&self) -> Result<QualityProfile, StorageError> {
        let rules: QualityRules = serde_json::from_str(&self.rules_json).map_err(|error| {
            StorageError::InvalidJsonField {
                column: "rules_json",
                detail: error.to_string(),
            }
        })?;
        let cutoff =
            u16::try_from(self.cutoff_rank).map_err(|_| StorageError::UnexpectedValue {
                column: "cutoff_rank",
                found: self.cutoff_rank.to_string(),
            })?;
        QualityProfile::new(
            &self.name,
            rules,
            QualityRank::new(cutoff),
            self.upgrade_allowed != 0,
            true,
        )
        .map_err(|error| StorageError::UnexpectedValue {
            column: "quality_profile",
            found: error.to_string(),
        })
    }
}
