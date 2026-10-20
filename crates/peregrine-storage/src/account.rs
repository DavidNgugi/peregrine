//! The household's profiles, which are the `account` rows the scope layer already names.
//!
//! A profile is an account with a display name and an avatar; the library is shared, while
//! watch progress, the watchlist, continue-watching, and notifications are keyed by the account
//! id. The active account is one installation-wide choice stored in `settings`, so the desktop
//! and every paired browser switch to the same person.
//!
//! The capability columns (kind, rating ceiling, curation, acquisition) are written now and
//! enforced by the scope's restricted visibility; the full parental gate is E35's.

use sqlx::SqlitePool;

use crate::StorageError;

/// One profile row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    /// The account id every scoped row carries.
    pub id: i64,
    /// The display name.
    pub name: String,
    /// `adult`, `child`, or `guest`.
    pub kind: String,
    /// `builtin`, `initials`, `custom`, or `default`.
    pub avatar_kind: String,
    /// The avatar's style and seed, or a data URL.
    pub avatar_ref: Option<String>,
    /// The highest certification the account may see, when it has one.
    pub rating_ceiling: Option<String>,
    /// Whether only explicitly curated titles are visible.
    pub curated_only: bool,
    /// Whether the account may start a download.
    pub can_acquire: bool,
    /// Whether this is the installation's first account.
    pub is_default: bool,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds.
    pub last_used_at: Option<i64>,
}

impl AccountRow {
    /// Whether this account's scope hides adult and unrated content.
    #[must_use]
    pub fn is_restricted(&self) -> bool {
        self.kind == "child" || self.rating_ceiling.is_some() || self.curated_only
    }
}

/// The columns one profile creation supplies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    /// The display name.
    pub name: String,
    /// `adult`, `child`, or `guest`.
    pub kind: String,
    /// `initials` for a generated avatar, `custom` for a data URL.
    pub avatar_kind: String,
    /// The avatar's seed/style or data URL.
    pub avatar_ref: Option<String>,
    /// The highest certification the account may see.
    pub rating_ceiling: Option<String>,
    /// Whether only curated titles are visible.
    pub curated_only: bool,
    /// Whether the account may start a download.
    pub can_acquire: bool,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// A profile's editable fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEdit {
    /// The new display name.
    pub name: String,
    /// `initials` or `custom`.
    pub avatar_kind: String,
    /// The avatar's seed/style or data URL.
    pub avatar_ref: Option<String>,
}

type AccountQueryRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
);

fn to_row(value: AccountQueryRow) -> AccountRow {
    AccountRow {
        id: value.0,
        name: value.1,
        kind: value.2,
        avatar_kind: value.3,
        avatar_ref: value.4,
        rating_ceiling: value.5,
        curated_only: value.6 != 0,
        can_acquire: value.7 != 0,
        is_default: value.8 != 0,
        created_at: value.9,
        last_used_at: value.10,
    }
}

/// Every profile, oldest first, with the default account ahead of the rest.
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<AccountRow>, StorageError> {
    let rows = sqlx::query_as::<_, AccountQueryRow>(
        "SELECT id, name, kind, avatar_kind, avatar_ref, rating_ceiling, curated_only, \
         can_acquire, is_default, created_at, last_used_at FROM account \
         ORDER BY is_default DESC, created_at, id",
    )
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(to_row).collect())
}

/// One profile by id.
pub(crate) async fn get(pool: &SqlitePool, id: i64) -> Result<Option<AccountRow>, StorageError> {
    let record = sqlx::query_as::<_, AccountQueryRow>(
        "SELECT id, name, kind, avatar_kind, avatar_ref, rating_ceiling, curated_only, \
         can_acquire, is_default, created_at, last_used_at FROM account WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(record.map(to_row))
}

/// Inserts one profile and returns its id.
pub(crate) async fn insert(
    connection: &mut sqlx::SqliteConnection,
    account: &NewAccount,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO account (name, kind, avatar_kind, avatar_ref, rating_ceiling, curated_only, \
         can_acquire, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&account.name)
    .bind(&account.kind)
    .bind(&account.avatar_kind)
    .bind(&account.avatar_ref)
    .bind(&account.rating_ceiling)
    .bind(i64::from(account.curated_only))
    .bind(i64::from(account.can_acquire))
    .bind(account.created_at)
    .execute(connection)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Writes a profile's name and avatar.
pub(crate) async fn update(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    edit: &AccountEdit,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE account SET name = ?1, avatar_kind = ?2, avatar_ref = ?3 WHERE id = ?4",
    )
    .bind(&edit.name)
    .bind(&edit.avatar_kind)
    .bind(&edit.avatar_ref)
    .bind(id)
    .execute(connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The account the installation last chose, when the setting names a live row.
pub(crate) async fn active(pool: &SqlitePool) -> Result<Option<AccountRow>, StorageError> {
    let value: Option<(String,)> = sqlx::query_as("SELECT value_json FROM settings WHERE key = ?1")
        .bind(ACTIVE_KEY)
        .fetch_optional(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    if let Some((value,)) = value
        && let Ok(id) = serde_json::from_str::<i64>(&value)
        && let Some(account) = get(pool, id).await?
    {
        return Ok(Some(account));
    }
    default(pool).await
}

/// The seeded account a fresh installation watches as before the picker is used.
async fn default(pool: &SqlitePool) -> Result<Option<AccountRow>, StorageError> {
    let record = sqlx::query_as::<_, AccountQueryRow>(
        "SELECT id, name, kind, avatar_kind, avatar_ref, rating_ceiling, curated_only, \
         can_acquire, is_default, created_at, last_used_at FROM account \
         WHERE is_default = 1 ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(record.map(to_row))
}

/// Remembers the chosen profile and stamps it as last used.
pub(crate) async fn set_active(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, \
         updated_at = excluded.updated_at",
    )
    .bind(ACTIVE_KEY)
    .bind(id.to_string())
    .bind(at)
    .execute(&mut *connection)
    .await?;
    sqlx::query("UPDATE account SET last_used_at = ?1 WHERE id = ?2")
        .bind(at)
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}

/// The setting key the active account is stored under.
pub(crate) const ACTIVE_KEY: &str = "active_account";

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::Database;

    fn account(name: &str, kind: &str) -> NewAccount {
        NewAccount {
            name: name.to_owned(),
            kind: kind.to_owned(),
            avatar_kind: "initials".to_owned(),
            avatar_ref: None,
            rating_ceiling: (kind == "child").then(|| "PG-13".to_owned()),
            curated_only: false,
            can_acquire: kind != "child",
            created_at: 10,
        }
    }

    #[tokio::test]
    async fn profiles_round_trip_and_the_active_choice_persists() {
        let directory = TempDir::new().unwrap();
        let database = Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();

        let seeded = database.profiles().await.unwrap();
        assert_eq!(seeded[0].name, "You");
        assert!(seeded[0].is_default);

        let id = database
            .create_profile(account("Ada", "adult"))
            .await
            .unwrap();
        let listed = database.profiles().await.unwrap();
        assert_eq!(listed.last().unwrap().name, "Ada");
        assert_eq!(
            database.active_profile().await.unwrap().unwrap().name,
            "You",
            "the seeded default is active before the picker is used"
        );

        database.set_active_profile(id, 20).await.unwrap();
        let active = database.active_profile().await.unwrap().unwrap();
        assert_eq!(active.id, id);
        assert_eq!(active.last_used_at, Some(20));
    }

    #[tokio::test]
    async fn a_child_profile_is_restricted_and_cannot_acquire() {
        let directory = TempDir::new().unwrap();
        let database = Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();
        let id = database
            .create_profile(account("Kid", "child"))
            .await
            .unwrap();
        let child = database.profile(id).await.unwrap().unwrap();
        assert!(child.is_restricted());
        assert!(!child.can_acquire);
    }

    #[tokio::test]
    async fn editing_a_profile_keeps_its_watch_state() {
        let directory = TempDir::new().unwrap();
        let database = Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();
        let id = database
            .create_profile(account("Ada", "adult"))
            .await
            .unwrap();
        let changed = database
            .update_profile(
                id,
                AccountEdit {
                    name: "Ada Lovelace".to_owned(),
                    avatar_kind: "custom".to_owned(),
                    avatar_ref: Some("data:image/png;base64,AA".to_owned()),
                },
            )
            .await
            .unwrap();
        assert!(changed);
        let row = database.profile(id).await.unwrap().unwrap();
        assert_eq!(row.name, "Ada Lovelace");
        assert_eq!(row.avatar_kind, "custom");
    }
}
