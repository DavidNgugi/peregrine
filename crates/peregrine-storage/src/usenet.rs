//! The Usenet provider and policy repositories.
//!
//! Two aggregates with one writer each: `usenet_server` is the provider a transfer contacts, and
//! `usenet_setting` is the one-row policy the gate reads. Neither holds a credential value: the
//! password column is the keyring key name, so a copied database is not a set of provider logins.

use jiff::Timestamp;
use peregrine_domain::{NonEmptyText, UsenetHealth};
use sqlx::SqlitePool;

use crate::error::StorageError;

/// One configured Usenet provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsenetServer {
    /// The row id.
    pub id: i64,
    /// The name a person gave it.
    pub name: NonEmptyText,
    /// The host to connect to.
    pub host: NonEmptyText,
    /// The port.
    pub port: u16,
    /// Whether the connection uses implicit TLS.
    pub tls: bool,
    /// The account name, which is configuration rather than a secret.
    pub username: String,
    /// The keyring key that holds the password.
    pub password_secret: NonEmptyText,
    /// How many connections the engine may open to it.
    pub connections: u16,
    /// Lower values are tried first; a second row is the backup.
    pub priority: i64,
    /// Whether the engine may contact it.
    pub enabled: bool,
    /// The last known health.
    pub health: UsenetHealth,
    /// When health was last checked.
    pub last_check_at: Option<Timestamp>,
    /// When the row was created.
    pub created_at: Timestamp,
}

/// The columns an insert or an update writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUsenetServer {
    /// The name a person gave it.
    pub name: NonEmptyText,
    /// The host to connect to.
    pub host: NonEmptyText,
    /// The port.
    pub port: u16,
    /// Whether the connection uses implicit TLS.
    pub tls: bool,
    /// The account name.
    pub username: String,
    /// The keyring key that holds the password.
    pub password_secret: NonEmptyText,
    /// How many connections the engine may open to it.
    pub connections: u16,
    /// Lower values are tried first.
    pub priority: i64,
    /// Whether the engine may contact it.
    pub enabled: bool,
    /// When the row was created.
    pub created_at: Timestamp,
}

/// The one-row Usenet policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsenetSetting {
    /// Whether an archive entry may be extracted for a Usenet payload.
    pub allow_archives: bool,
    /// Whether the archives are removed after a successful import.
    pub delete_archives_after_import: bool,
}

impl Default for UsenetSetting {
    fn default() -> Self {
        Self {
            allow_archives: false,
            delete_archives_after_import: true,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ServerRow {
    id: i64,
    name: String,
    host: String,
    port: i64,
    tls: i64,
    username: String,
    password_secret: String,
    connections: i64,
    priority: i64,
    enabled: i64,
    health_state: String,
    last_check_at: Option<i64>,
    created_at: i64,
}

impl TryFrom<ServerRow> for UsenetServer {
    type Error = StorageError;

    fn try_from(row: ServerRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            name: text(row.name, "usenet_server.name")?,
            host: text(row.host, "usenet_server.host")?,
            port: port(row.port)?,
            tls: row.tls != 0,
            username: row.username,
            password_secret: text(row.password_secret, "usenet_server.password_secret")?,
            connections: connections(row.connections)?,
            priority: row.priority,
            enabled: row.enabled != 0,
            health: UsenetHealth::parse(&row.health_state).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "usenet_server.health_state",
                    found: row.health_state.clone(),
                }
            })?,
            last_check_at: row.last_check_at.map(millis),
            created_at: millis(row.created_at),
        })
    }
}

fn text(value: String, column: &'static str) -> Result<NonEmptyText, StorageError> {
    NonEmptyText::new(&value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value,
    })
}

fn port(value: i64) -> Result<u16, StorageError> {
    u16::try_from(value)
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| StorageError::UnexpectedValue {
            column: "usenet_server.port",
            found: value.to_string(),
        })
}

fn connections(value: i64) -> Result<u16, StorageError> {
    u16::try_from(value)
        .ok()
        .filter(|count| (1..=64).contains(count))
        .ok_or_else(|| StorageError::UnexpectedValue {
            column: "usenet_server.connections",
            found: value.to_string(),
        })
}

fn millis(value: i64) -> Timestamp {
    Timestamp::from_millisecond(value).unwrap_or(Timestamp::UNIX_EPOCH)
}

fn as_millis(value: Timestamp) -> i64 {
    value.as_millisecond()
}

fn flag(value: bool) -> i64 {
    i64::from(value)
}

const LIST_SQL: &str = "SELECT id, name, host, port, tls, username, password_secret, connections, \
     priority, enabled, health_state, last_check_at, created_at FROM usenet_server \
     ORDER BY priority, id";
const GET_SQL: &str = "SELECT id, name, host, port, tls, username, password_secret, connections, \
     priority, enabled, health_state, last_check_at, created_at FROM usenet_server WHERE id = ?1";

/// Every provider, primary first.
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<UsenetServer>, StorageError> {
    let rows = sqlx::query_as::<_, ServerRow>(LIST_SQL)
        .fetch_all(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// One provider by id.
pub(crate) async fn get(pool: &SqlitePool, id: i64) -> Result<Option<UsenetServer>, StorageError> {
    sqlx::query_as::<_, ServerRow>(GET_SQL)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(StorageError::from_sqlx)?
        .map(TryInto::try_into)
        .transpose()
}

/// Inserts one provider and returns the row the database holds.
pub(crate) async fn insert(
    connection: &mut sqlx::SqliteConnection,
    server: &NewUsenetServer,
) -> Result<UsenetServer, StorageError> {
    let result = sqlx::query(
        "INSERT INTO usenet_server (name, host, port, tls, username, password_secret, \
         connections, priority, enabled, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(server.name.as_str())
    .bind(server.host.as_str())
    .bind(i64::from(server.port))
    .bind(flag(server.tls))
    .bind(&server.username)
    .bind(server.password_secret.as_str())
    .bind(i64::from(server.connections))
    .bind(server.priority)
    .bind(flag(server.enabled))
    .bind(as_millis(server.created_at))
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    read_by_id(&mut *connection, result.last_insert_rowid()).await
}

/// Replaces the editable columns of one provider, answering the row or `None`.
pub(crate) async fn update(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    server: &NewUsenetServer,
) -> Result<Option<UsenetServer>, StorageError> {
    let result = sqlx::query(
        "UPDATE usenet_server SET name = ?1, host = ?2, port = ?3, tls = ?4, username = ?5, \
         password_secret = ?6, connections = ?7, priority = ?8, enabled = ?9 WHERE id = ?10",
    )
    .bind(server.name.as_str())
    .bind(server.host.as_str())
    .bind(i64::from(server.port))
    .bind(flag(server.tls))
    .bind(&server.username)
    .bind(server.password_secret.as_str())
    .bind(i64::from(server.connections))
    .bind(server.priority)
    .bind(flag(server.enabled))
    .bind(id)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    read_by_id(&mut *connection, id).await.map(Some)
}

/// Deletes one provider, answering whether it was there.
pub(crate) async fn delete(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query("DELETE FROM usenet_server WHERE id = ?1")
        .bind(id)
        .execute(&mut *connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Records one health answer, answering whether the row was there.
pub(crate) async fn record_health(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    health: UsenetHealth,
    at: Timestamp,
) -> Result<bool, StorageError> {
    let result =
        sqlx::query("UPDATE usenet_server SET health_state = ?1, last_check_at = ?2 WHERE id = ?3")
            .bind(health.as_str())
            .bind(as_millis(at))
            .bind(id)
            .execute(&mut *connection)
            .await
            .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// The one policy row.
pub(crate) async fn setting(pool: &SqlitePool) -> Result<UsenetSetting, StorageError> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT allow_archives, delete_archives_after_import FROM usenet_setting WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(UsenetSetting {
        allow_archives: row.0 != 0,
        delete_archives_after_import: row.1 != 0,
    })
}

/// Writes the one policy row and answers what it now holds.
pub(crate) async fn set_setting(
    connection: &mut sqlx::SqliteConnection,
    setting: UsenetSetting,
    at: Timestamp,
) -> Result<UsenetSetting, StorageError> {
    sqlx::query(
        "UPDATE usenet_setting SET allow_archives = ?1, delete_archives_after_import = ?2, \
         updated_at = ?3 WHERE id = 1",
    )
    .bind(flag(setting.allow_archives))
    .bind(flag(setting.delete_archives_after_import))
    .bind(as_millis(at))
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(setting)
}

async fn read_by_id(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<UsenetServer, StorageError> {
    sqlx::query_as::<_, ServerRow>(GET_SQL)
        .bind(id)
        .fetch_one(connection)
        .await
        .map_err(StorageError::from_sqlx)?
        .try_into()
}

/// The writer request a Usenet settings change carries.
#[derive(Debug)]
pub(crate) enum Write {
    /// Insert one provider.
    Insert(NewUsenetServer),
    /// Replace one provider's editable columns.
    Update { id: i64, server: NewUsenetServer },
    /// Delete one provider.
    Delete { id: i64 },
    /// Record one health answer.
    Health {
        id: i64,
        health: UsenetHealth,
        at: Timestamp,
    },
    /// Replace the one policy row.
    Setting {
        setting: UsenetSetting,
        at: Timestamp,
    },
}

/// What a Usenet settings write answered.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The inserted row.
    Inserted(UsenetServer),
    /// The updated row, or `None` when it was not there.
    Updated(Option<UsenetServer>),
    /// Whether a delete removed a row.
    Deleted(bool),
    /// Whether a health answer found the row.
    Health(bool),
    /// The policy row as written.
    Setting(UsenetSetting),
}

pub(crate) async fn apply(
    connection: &mut sqlx::SqliteConnection,
    write: Write,
) -> Result<Outcome, StorageError> {
    match write {
        Write::Insert(server) => insert(connection, &server).await.map(Outcome::Inserted),
        Write::Update { id, server } => update(connection, id, &server).await.map(Outcome::Updated),
        Write::Delete { id } => delete(connection, id).await.map(Outcome::Deleted),
        Write::Health { id, health, at } => record_health(connection, id, health, at)
            .await
            .map(Outcome::Health),
        Write::Setting { setting, at } => set_setting(connection, setting, at)
            .await
            .map(Outcome::Setting),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::database;
    use tempfile::TempDir;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("the fixture instant is in range")
    }

    fn server(name: &str) -> NewUsenetServer {
        NewUsenetServer {
            name: NonEmptyText::new(name).expect("a name"),
            host: NonEmptyText::new("news.example.test").expect("a host"),
            port: 563,
            tls: true,
            username: "reader".to_owned(),
            password_secret: NonEmptyText::new("provider.usenet.one").expect("a key"),
            connections: 8,
            priority: 10,
            enabled: true,
            created_at: at(1_700_000_000_000),
        }
    }

    #[tokio::test]
    async fn a_provider_round_trips_and_health_is_recorded() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let created = database
            .create_usenet_server(server("Primary"))
            .await
            .expect("inserts");
        assert_eq!(created.port, 563);
        assert_eq!(created.connections, 8);
        assert_eq!(created.health, UsenetHealth::Unknown);

        let mut changed = server("Backup");
        changed.priority = 20;
        changed.enabled = false;
        let updated = database
            .update_usenet_server(created.id, changed)
            .await
            .unwrap()
            .expect("the row is there");
        assert_eq!(updated.name.as_str(), "Backup");
        assert!(!updated.enabled);

        assert!(
            database
                .record_usenet_health(created.id, UsenetHealth::Ok, at(1_700_000_100_000))
                .await
                .unwrap()
        );
        let read = database
            .usenet_server(created.id)
            .await
            .unwrap()
            .expect("the row is there");
        assert_eq!(read.health, UsenetHealth::Ok);
        assert_eq!(read.last_check_at, Some(at(1_700_000_100_000)));

        assert_eq!(database.usenet_servers().await.unwrap().len(), 1);
        assert!(database.delete_usenet_server(created.id).await.unwrap());
        assert_eq!(database.usenet_servers().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn the_policy_defaults_to_archives_off_and_round_trips() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        assert_eq!(
            database.usenet_setting().await.unwrap(),
            UsenetSetting::default()
        );

        let written = database
            .set_usenet_setting(
                UsenetSetting {
                    allow_archives: true,
                    delete_archives_after_import: false,
                },
                at(1_700_000_000_000),
            )
            .await
            .unwrap();
        assert!(written.allow_archives);
        assert!(!written.delete_archives_after_import);
        assert_eq!(database.usenet_setting().await.unwrap(), written);
    }
}
