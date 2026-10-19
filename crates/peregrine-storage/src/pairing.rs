//! Devices paired to the LAN surface (ADR 0028).
//!
//! The token is a credential this application mints, not one it consumes, so a row stores
//! `sha256(token)` and never the token: a copy of the database is a list of devices, not a set of
//! working credentials. Revocation is a timestamp rather than a delete, because "which device was
//! cut off, and when" is the question an audit asks and a deleted row cannot answer it.
//!
//! This module is deliberately storage-only. The scope vocabulary, the code ceremony, and the
//! decision that a token is accepted belong to `peregrine-app::web`; what is here is the row.

use sqlx::SqlitePool;

use crate::StorageError;

/// A device that may present a token to the LAN surface.
///
/// The token hash is deliberately absent: a reader of the device list never needs it, and a type
/// that carried it would put the one value worth stealing in every `Debug` line that prints a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// The row id.
    pub id: i64,
    /// The name the person gave it, or the name the client offered at pairing.
    pub name: String,
    /// What kind of client it says it is.
    pub platform: Option<String>,
    /// The scopes it was issued, as the column holds them.
    pub scopes: String,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds of the last authenticated request, when one has been seen.
    pub last_seen_at: Option<i64>,
    /// The address it last presented, for the device list.
    pub last_address: Option<String>,
    /// Unix milliseconds at which the person revoked it, when they have.
    pub revoked_at: Option<i64>,
}

/// A device to write at pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPairedDevice {
    /// The display name, already trimmed and bounded by the caller.
    pub name: String,
    /// The client's platform, when it named one.
    pub platform: Option<String>,
    /// `sha256(token)`, lowercase hex.
    pub token_hash: String,
    /// The scopes issued, as the column stores them.
    pub scopes: String,
    /// The address the pairing request came from.
    pub address: Option<String>,
    /// Unix milliseconds.
    pub at: i64,
}

/// Writes one paired device and answers its row id.
pub(crate) async fn insert(
    connection: &mut sqlx::SqliteConnection,
    device: &NewPairedDevice,
) -> Result<i64, StorageError> {
    let result = sqlx::query(
        "INSERT INTO paired_device (name, platform, token_hash, scopes, created_at, last_seen_at, last_address, revoked_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL)",
    )
    .bind(&device.name)
    .bind(device.platform.as_deref())
    .bind(&device.token_hash)
    .bind(&device.scopes)
    .bind(device.at)
    .bind(device.address.as_deref())
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.last_insert_rowid())
}

/// Records that a device was seen, answering whether the row is still there and not revoked.
pub(crate) async fn touch(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    at: i64,
    address: Option<&str>,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "UPDATE paired_device SET last_seen_at = ?2, last_address = COALESCE(?3, last_address) WHERE id = ?1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(at)
    .bind(address)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Revokes a device, answering whether this call was the one that revoked it.
pub(crate) async fn revoke(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    at: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "UPDATE paired_device SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(at)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// One device by its row id, revoked or not.
pub(crate) async fn by_id(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<PairedDevice>, StorageError> {
    let row = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, platform, scopes, created_at, last_seen_at, last_address, revoked_at FROM paired_device WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(Into::into))
}

/// The device a token hash belongs to, revoked or not.
///
/// Revocation is returned rather than filtered here, because the two callers want different
/// answers: authentication refuses a revoked row, and the device list draws it.
pub(crate) async fn for_token(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<PairedDevice>, StorageError> {
    let row = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, platform, scopes, created_at, last_seen_at, last_address, revoked_at FROM paired_device WHERE token_hash = ?1",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(Into::into))
}

/// Every paired device, oldest first, so a stable list needs no second sort.
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<PairedDevice>, StorageError> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, platform, scopes, created_at, last_seen_at, last_address, revoked_at FROM paired_device ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: i64,
    name: String,
    platform: Option<String>,
    scopes: String,
    created_at: i64,
    last_seen_at: Option<i64>,
    last_address: Option<String>,
    revoked_at: Option<i64>,
}

impl From<DeviceRow> for PairedDevice {
    fn from(row: DeviceRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            platform: row.platform,
            scopes: row.scopes,
            created_at: row.created_at,
            last_seen_at: row.last_seen_at,
            last_address: row.last_address,
            revoked_at: row.revoked_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::NewPairedDevice;
    use crate::test_support::{NOW, database};

    fn new_device(hash: char, at: i64) -> NewPairedDevice {
        NewPairedDevice {
            name: "Living room TV".to_owned(),
            platform: Some("Tizen".to_owned()),
            token_hash: hash.to_string().repeat(64),
            scopes: "read,stream".to_owned(),
            address: Some("192.168.1.42".to_owned()),
            at,
        }
    }

    #[tokio::test]
    async fn a_paired_device_round_trips_without_storing_its_token() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database
            .insert_paired_device(new_device('a', NOW))
            .await
            .unwrap();

        let found = database
            .paired_device_for_token_hash(&"a".repeat(64))
            .await
            .unwrap()
            .expect("the device is there");

        assert_eq!(found.id, id);
        assert_eq!(found.name, "Living room TV");
        assert_eq!(found.platform.as_deref(), Some("Tizen"));
        assert_eq!(found.scopes, "read,stream");
        assert_eq!(found.last_seen_at, None);
        assert_eq!(found.last_address.as_deref(), Some("192.168.1.42"));
        assert_eq!(found.revoked_at, None);
    }

    #[tokio::test]
    async fn touching_records_the_instant_and_answers_for_an_active_row() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database
            .insert_paired_device(new_device('b', NOW))
            .await
            .unwrap();

        assert!(
            database
                .touch_paired_device(id, NOW + 1_000, Some("192.168.1.9"))
                .await
                .unwrap()
        );
        let found = database
            .paired_device_for_token_hash(&"b".repeat(64))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.last_seen_at, Some(NOW + 1_000));
        assert_eq!(found.last_address.as_deref(), Some("192.168.1.9"));

        assert!(
            !database.touch_paired_device(999, NOW, None).await.unwrap(),
            "an id no row holds is not a successful touch"
        );
    }

    #[tokio::test]
    async fn revoking_keeps_the_row_and_is_reported_once() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database
            .insert_paired_device(new_device('c', NOW))
            .await
            .unwrap();

        assert!(database.revoke_paired_device(id, NOW + 5).await.unwrap());
        assert!(
            !database.revoke_paired_device(id, NOW + 6).await.unwrap(),
            "the second revoke changed nothing, and says so"
        );

        let found = database
            .paired_device_for_token_hash(&"c".repeat(64))
            .await
            .unwrap()
            .expect("a revoked row stays for the list and the audit");
        assert_eq!(found.revoked_at, Some(NOW + 5));
        assert!(
            !database
                .touch_paired_device(id, NOW + 7, None)
                .await
                .unwrap(),
            "a revoked device is not seen again"
        );
    }

    #[tokio::test]
    async fn an_unknown_hash_answers_none_and_the_list_is_oldest_first() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        assert!(
            database
                .paired_device_for_token_hash(&"f".repeat(64))
                .await
                .unwrap()
                .is_none()
        );

        database
            .insert_paired_device(new_device('d', NOW))
            .await
            .unwrap();
        database
            .insert_paired_device(new_device('e', NOW + 1))
            .await
            .unwrap();

        let rows = database.paired_devices().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].id < rows[1].id);
    }
}
