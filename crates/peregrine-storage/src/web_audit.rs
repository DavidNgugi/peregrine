//! The audit of acquisitions a paired device asked for (ADR 0028).
//!
//! The desktop queue records what was downloaded; this records who asked. The device row
//! outlives revocation, so the audit still names the device after its credential is cut off.

use sqlx::SqliteConnection;

use crate::StorageError;

/// What a paired device did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebAuditAction {
    /// It asked the LAN surface to start an acquisition.
    Acquire,
}

impl WebAuditAction {
    /// The spelling the `action` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acquire => "acquire",
        }
    }
}

/// One audit row to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWebAudit {
    /// The paired device that asked.
    pub device_id: i64,
    /// What it asked for.
    pub action: WebAuditAction,
    /// The catalogue title.
    pub title_id: i64,
    /// The episode, when the request named one.
    pub episode_id: Option<i64>,
    /// The journal job the request started, when it started one.
    pub job_id: Option<i64>,
    /// Unix milliseconds.
    pub at: i64,
}

/// Writes one audit row and answers its id.
pub(crate) async fn insert(
    connection: &mut SqliteConnection,
    audit: &NewWebAudit,
) -> Result<i64, StorageError> {
    let result = sqlx::query(
        "INSERT INTO web_audit (device_id, action, title_id, episode_id, job_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(audit.device_id)
    .bind(audit.action.as_str())
    .bind(audit.title_id)
    .bind(audit.episode_id)
    .bind(audit.job_id)
    .bind(audit.at)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.last_insert_rowid())
}

/// Records the journal job an audited request started.
pub(crate) async fn set_job(
    connection: &mut SqliteConnection,
    id: i64,
    job_id: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query("UPDATE web_audit SET job_id = ?1 WHERE id = ?2")
        .bind(job_id)
        .bind(id)
        .execute(connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{NewWebAudit, WebAuditAction};
    use crate::pairing::NewPairedDevice;
    use crate::test_support::{NOW, database, path_of};

    #[tokio::test]
    async fn an_acquisition_is_audited_against_its_device() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let device = database
            .insert_paired_device(NewPairedDevice {
                name: "Living room TV".to_owned(),
                platform: None,
                token_hash: "a".repeat(64),
                scopes: "read,stream,acquire".to_owned(),
                address: None,
                at: NOW,
            })
            .await
            .unwrap();
        let id = database
            .record_web_audit(NewWebAudit {
                device_id: device,
                action: WebAuditAction::Acquire,
                title_id: 7,
                episode_id: None,
                job_id: None,
                at: NOW,
            })
            .await
            .unwrap();
        assert!(id > 0);
        assert_eq!(
            crate::test_support::count(&path_of(&directory), "SELECT count(*) FROM web_audit")
                .await,
            1
        );
    }
}
