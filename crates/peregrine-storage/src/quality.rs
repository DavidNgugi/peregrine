//! The quality profile a release is chosen against.
//!
//! The row is three scalars plus a JSON document, so a profile is assembled here rather than read
//! as one blob: rules_json is only the rules, and the name, cutoff, and upgrade flag are columns
//! the profile type also wants. Parsing through the domain's own Deserialize is what keeps a
//! hand-edited file from widening a preference order the constructor would have refused.

use peregrine_domain::{QualityProfile, QualityRank, QualityRules};
use sqlx::SqlitePool;

use crate::error::StorageError;

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
