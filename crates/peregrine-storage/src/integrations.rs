//! Integration choices and the health each bounded probe last reported.
//!
//! One row per integration, written by the settings screen and read by the settings screen, the
//! read-only TV status, and the marker resolver. The credential is not here: `peregrine-secrets`
//! owns it, and this table stores only the decision about the integration.
//!
//! **An absent row is not an error.** Every integration starts with no row, and the manifest's
//! defaults answer until a person changes one. Writing a row is therefore an upsert keyed on the
//! integration id rather than an insert.

use peregrine_domain::{IntegrationId, IntegrationOutcome, IntegrationPreference};
use sqlx::{SqliteConnection, SqlitePool};

use crate::error::StorageError;

/// One integration's stored choice, plus the last probe's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIntegration {
    /// What the person chose.
    pub preference: IntegrationPreference,
    /// When the last bounded probe ran, in Unix milliseconds.
    pub last_test_at: Option<i64>,
    /// What it answered, when it has ever run.
    pub last_outcome: Option<IntegrationOutcome>,
    /// The stable code the job recorded, when the last probe failed.
    pub last_error_code: Option<String>,
}

/// Reads every stored integration, newest write first is not meaningful here: the caller orders
/// by its own manifest, so this is an unordered keyed read.
pub(crate) async fn read_all(pool: &SqlitePool) -> Result<Vec<StoredIntegration>, StorageError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            i64,
            i64,
            i64,
            Option<i64>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT integration, enabled, priority, submission_enabled, last_test_at, last_outcome, \
         last_error_code FROM integration_setting",
    )
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    rows.into_iter()
        .map(
            |(
                integration,
                enabled,
                priority,
                submission_enabled,
                last_test_at,
                last_outcome,
                last_error_code,
            )| {
                let id = IntegrationId::new(&integration).map_err(|_| {
                    StorageError::UnexpectedValue {
                        column: "integration_setting.integration",
                        found: integration.clone(),
                    }
                })?;
                let outcome = match last_outcome {
                    Some(value) => Some(IntegrationOutcome::parse(&value).ok_or_else(|| {
                        StorageError::UnexpectedValue {
                            column: "integration_setting.last_outcome",
                            found: value.clone(),
                        }
                    })?),
                    None => None,
                };
                Ok(StoredIntegration {
                    preference: IntegrationPreference {
                        id,
                        enabled: enabled != 0,
                        priority: u32::try_from(priority).map_err(|_| {
                            StorageError::UnexpectedValue {
                                column: "integration_setting.priority",
                                found: priority.to_string(),
                            }
                        })?,
                        submission_enabled: submission_enabled != 0,
                    },
                    last_test_at,
                    last_outcome: outcome,
                    last_error_code,
                })
            },
        )
        .collect()
}

/// Writes one integration's choice, leaving the last probe's health untouched.
pub(crate) async fn upsert(
    connection: &mut SqliteConnection,
    preference: &IntegrationPreference,
    at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO integration_setting (integration, enabled, priority, submission_enabled, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT (integration) DO UPDATE SET \
             enabled = excluded.enabled, priority = excluded.priority, \
             submission_enabled = excluded.submission_enabled, updated_at = excluded.updated_at",
    )
    .bind(preference.id.as_str())
    .bind(i64::from(preference.enabled))
    .bind(i64::from(preference.priority))
    .bind(i64::from(preference.submission_enabled))
    .bind(at)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Records what one probe answered, creating the row when a person has never changed the default.
///
/// The row's other columns are left as they are: a probe must not re-enable an integration a person
/// turned off, and it must not change its priority.
pub(crate) async fn record_outcome(
    connection: &mut SqliteConnection,
    id: &IntegrationId,
    outcome: IntegrationOutcome,
    error_code: Option<&str>,
    at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO integration_setting (integration, enabled, priority, submission_enabled, \
         last_test_at, last_outcome, last_error_code, updated_at) \
         VALUES (?1, 0, 0, 0, ?2, ?3, ?4, ?2) \
         ON CONFLICT (integration) DO UPDATE SET \
             last_test_at = excluded.last_test_at, last_outcome = excluded.last_outcome, \
             last_error_code = excluded.last_error_code, updated_at = excluded.updated_at",
    )
    .bind(id.as_str())
    .bind(at)
    .bind(outcome.as_str())
    .bind(error_code)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Forgets one integration's choice and health.
///
/// Removing credentials is a separate act from disabling the integration, so this is deliberately
/// not wired to the remove-credentials path: a person who clears a key keeps the card's position.
pub(crate) async fn forget(
    connection: &mut SqliteConnection,
    id: &IntegrationId,
) -> Result<bool, StorageError> {
    let result = sqlx::query("DELETE FROM integration_setting WHERE integration = ?1")
        .bind(id.as_str())
        .execute(connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use peregrine_domain::{IntegrationId, IntegrationOutcome};

    use super::*;

    fn preference(id: &str, enabled: bool, priority: u32) -> IntegrationPreference {
        IntegrationPreference {
            id: IntegrationId::new(id).unwrap(),
            enabled,
            priority,
            submission_enabled: false,
        }
    }

    #[tokio::test]
    async fn an_installation_with_no_rows_answers_an_empty_list() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;

        assert!(database.integration_settings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_choice_round_trips_and_an_upsert_replaces_it() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        let first = preference("theintrodb", false, 0);
        let second = preference("theintrodb", true, 7);
        database
            .set_integration(&first, jiff::Timestamp::UNIX_EPOCH)
            .await
            .unwrap();
        database
            .set_integration(&second, jiff::Timestamp::UNIX_EPOCH)
            .await
            .unwrap();

        let stored = database.integration_settings().await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].preference, second);
    }

    #[tokio::test]
    async fn a_probe_does_not_re_enable_an_integration_or_change_its_priority() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        let chosen = preference("aniskip", false, 4);
        database
            .set_integration(&chosen, jiff::Timestamp::UNIX_EPOCH)
            .await
            .unwrap();
        database
            .record_integration_outcome(
                &chosen.id,
                IntegrationOutcome::RateLimited,
                Some("PROVIDER_RATE_LIMITED"),
                5,
            )
            .await
            .unwrap();

        let stored = database.integration_settings().await.unwrap();
        assert_eq!(stored[0].preference, chosen);
        assert_eq!(
            stored[0].last_outcome,
            Some(IntegrationOutcome::RateLimited)
        );
        assert_eq!(stored[0].last_test_at, Some(5));
        assert_eq!(
            stored[0].last_error_code.as_deref(),
            Some("PROVIDER_RATE_LIMITED")
        );
    }

    #[tokio::test]
    async fn a_probe_before_a_choice_creates_a_row_that_is_disabled() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        let id = IntegrationId::new("theintrodb").unwrap();
        database
            .record_integration_outcome(&id, IntegrationOutcome::Working, None, 1)
            .await
            .unwrap();

        let stored = database.integration_settings().await.unwrap();
        assert!(!stored[0].preference.enabled);
        assert_eq!(stored[0].last_outcome, Some(IntegrationOutcome::Working));
    }

    #[tokio::test]
    async fn forgetting_an_integration_removes_its_row() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        let chosen = preference("chapters", true, 0);
        database
            .set_integration(&chosen, jiff::Timestamp::UNIX_EPOCH)
            .await
            .unwrap();
        assert!(database.forget_integration(&chosen.id).await.unwrap());
        assert!(database.integration_settings().await.unwrap().is_empty());
        assert!(!database.forget_integration(&chosen.id).await.unwrap());
    }
}
