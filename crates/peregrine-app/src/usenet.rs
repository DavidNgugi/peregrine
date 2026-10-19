//! Usenet provider configuration: database rows to the engine's connection settings, and the
//! settings surface's read/write/test operations.
//!
//! The password is resolved from the keyring here, so the engine never reads the database and the
//! database never holds a credential value. A provider that is disabled is not offered at all; a
//! keyring that refuses is a typed failure rather than an empty password.

use std::sync::Arc;

use jiff::Timestamp;
use peregrine_domain::UsenetHealth;
use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};
pub use peregrine_storage::UsenetServer as ConfiguredUsenetServer;
use peregrine_storage::{Database, NewUsenetServer, StorageError, UsenetServer, UsenetSetting};
use peregrine_usenet::{NntpConfig, NntpConnection, ServerConfig};
use secrecy::SecretString;

/// Why the provider list or one settings operation failed.
#[derive(Debug, thiserror::Error)]
pub enum UsenetConfigError {
    /// The database refused the read or the write.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The keyring refused a password.
    #[error("the Usenet provider's password could not be read")]
    Secret(#[from] SecretStoreError),
    /// No enabled provider exists.
    #[error("no Usenet provider is configured")]
    Unconfigured,
    /// The provider refused the test connection.
    #[error("the provider could not be reached: {0}")]
    Transport(String),
    /// A field is outside what the schema accepts.
    #[error("that provider setting is not usable: {0}")]
    Invalid(&'static str),
}

/// What the settings screen reads.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // Four independent yes/no facts the settings screen shows.
pub struct UsenetStatus {
    /// Whether at least one enabled provider exists.
    pub configured: bool,
    /// The providers, primary first. Password values are never included.
    pub servers: Vec<UsenetServer>,
    /// Whether Usenet archives may be extracted.
    pub allow_archives: bool,
    /// Whether the archives are removed after a successful import.
    pub delete_archives_after_import: bool,
    /// Whether the archive extractor was found.
    pub extract_available: bool,
    /// Whether the repair tool was found.
    pub repair_available: bool,
}

/// The fields a settings save carries.
#[derive(Debug, Clone)]
pub struct UsenetServerInput {
    /// The row being edited, or None for a new provider.
    pub id: Option<i64>,
    /// The display name.
    pub name: String,
    /// The host.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Whether to use implicit TLS.
    pub tls: bool,
    /// The account name.
    pub username: String,
    /// The new password, when the user typed one.
    pub password: Option<String>,
    /// The connection count.
    pub connections: u16,
    /// Lower is tried first.
    pub priority: i64,
    /// Whether the engine may contact it.
    pub enabled: bool,
}

/// The enabled providers, as the engine wants them.
///
/// # Errors
///
/// `UsenetConfigError` when the rows or a password cannot be read, or when nothing is enabled.
pub async fn server_configs(
    database: &Database,
    store: Arc<dyn SecretStore>,
) -> Result<Vec<ServerConfig>, UsenetConfigError> {
    let rows = database.usenet_servers().await?;
    let mut servers = Vec::new();
    for row in rows.into_iter().filter(|row| row.enabled) {
        let password = read_password(Arc::clone(&store), &row.password_secret).await?;
        servers.push(ServerConfig {
            nntp: NntpConfig {
                host: row.host.as_str().to_owned(),
                port: row.port,
                tls: row.tls,
                username: row.username,
                password,
            },
            connections: u32::from(row.connections),
            priority: row.priority,
        });
    }
    if servers.is_empty() {
        return Err(UsenetConfigError::Unconfigured);
    }
    Ok(servers)
}

/// The settings screen's read.
pub async fn status(
    database: &Database,
    _store: Arc<dyn SecretStore>,
) -> Result<UsenetStatus, UsenetConfigError> {
    let servers = database.usenet_servers().await?;
    let setting = database.usenet_setting().await?;
    let tools = crate::usenet_post::PostTools::resolve();
    Ok(UsenetStatus {
        configured: servers.iter().any(|server| server.enabled),
        servers,
        allow_archives: setting.allow_archives,
        delete_archives_after_import: setting.delete_archives_after_import,
        extract_available: tools.is_some(),
        repair_available: tools.as_ref().is_some_and(|tools| tools.par2.is_some()),
    })
}

/// Inserts or updates one provider, writing the password to the keyring when one was typed.
pub async fn save(
    database: &Database,
    store: Arc<dyn SecretStore>,
    input: UsenetServerInput,
    at: Timestamp,
) -> Result<UsenetStatus, UsenetConfigError> {
    if input.port == 0 {
        return Err(UsenetConfigError::Invalid(
            "the port must be between 1 and 65535",
        ));
    }
    if !(1..=64).contains(&input.connections) {
        return Err(UsenetConfigError::Invalid(
            "connections must be between 1 and 64",
        ));
    }
    let name = non_empty(&input.name, "the name cannot be empty")?;
    let generated_key = format!("usenet.{}", input.name.trim());
    let host = non_empty(&input.host, "the host cannot be empty")?;

    let existing = match input.id {
        Some(id) => database.usenet_server(id).await?,
        None => None,
    };
    let secret = existing.as_ref().map_or_else(
        || {
            peregrine_domain::NonEmptyText::new(&generated_key)
                .expect("a generated key is not empty")
        },
        |server| server.password_secret.clone(),
    );
    let server = NewUsenetServer {
        name,
        host,
        port: input.port,
        tls: input.tls,
        username: input.username,
        password_secret: secret.clone(),
        connections: input.connections,
        priority: input.priority,
        enabled: input.enabled,
        created_at: at,
    };
    if let Some(secret_value) = input.password.as_deref() {
        let key = SecretKey::provider(secret.as_str());
        let value = Secret::new(secret_value.to_owned());
        let writer = Arc::clone(&store);
        tokio::task::spawn_blocking(move || writer.set(&key, &value))
            .await
            .map_err(|_| SecretStoreError::Failed)??;
    }
    match input.id {
        Some(id) => {
            database.update_usenet_server(id, server).await?;
        }
        None => {
            database.create_usenet_server(server).await?;
        }
    }
    status(database, store).await
}

/// Removes one provider and its keyring entry.
pub async fn remove(
    database: &Database,
    store: Arc<dyn SecretStore>,
    id: i64,
) -> Result<UsenetStatus, UsenetConfigError> {
    if let Some(server) = database.usenet_server(id).await? {
        let key = SecretKey::provider(server.password_secret.as_str());
        let writer = Arc::clone(&store);
        tokio::task::spawn_blocking(move || writer.delete(&key))
            .await
            .map_err(|_| SecretStoreError::Failed)??;
        database.delete_usenet_server(id).await?;
    }
    status(database, store).await
}

/// Tests one provider: the stored one when it has an id, otherwise the typed values.
pub async fn test(
    database: &Database,
    store: Arc<dyn SecretStore>,
    input: UsenetServerInput,
    at: Timestamp,
) -> Result<(), UsenetConfigError> {
    let password = match input.id {
        Some(id) => match database.usenet_server(id).await? {
            Some(server) => read_password(Arc::clone(&store), &server.password_secret).await?,
            None => None,
        },
        None => input
            .password
            .as_ref()
            .map(|value| SecretString::from(value.clone())),
    };
    let config = NntpConfig {
        host: input.host,
        port: input.port,
        tls: input.tls,
        username: input.username,
        password,
    };
    let connection = NntpConnection::connect(&config)
        .await
        .map_err(|error| UsenetConfigError::Transport(error.to_string()))?;
    connection.quit().await;
    if let Some(id) = input.id {
        database
            .record_usenet_health(id, UsenetHealth::Ok, at)
            .await?;
    }
    Ok(())
}

/// Writes the archive policy.
pub async fn set_archives(
    database: &Database,
    allow_archives: bool,
    delete_archives_after_import: bool,
    at: Timestamp,
) -> Result<UsenetSetting, UsenetConfigError> {
    Ok(database
        .set_usenet_setting(
            UsenetSetting {
                allow_archives,
                delete_archives_after_import,
            },
            at,
        )
        .await?)
}

async fn read_password(
    store: Arc<dyn SecretStore>,
    secret: &peregrine_domain::NonEmptyText,
) -> Result<Option<SecretString>, UsenetConfigError> {
    let key = SecretKey::provider(secret.as_str());
    let reader = Arc::clone(&store);
    let found = tokio::task::spawn_blocking(move || reader.get(&key))
        .await
        .map_err(|_| SecretStoreError::Failed)??;
    Ok(found.map(|value| SecretString::from(value.expose_secret().to_owned())))
}

fn non_empty(
    value: &str,
    reason: &'static str,
) -> Result<peregrine_domain::NonEmptyText, UsenetConfigError> {
    peregrine_domain::NonEmptyText::new(value).map_err(|_| UsenetConfigError::Invalid(reason))
}
#[cfg(test)]
mod tests {
    use super::*;
    use peregrine_testkit::MemorySecretStore;
    use tempfile::TempDir;

    async fn database() -> (TempDir, Database) {
        let directory = TempDir::new().expect("a temporary directory");
        let database = Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .expect("the database opens");
        (directory, database)
    }

    fn input(port: u16, connections: u16) -> UsenetServerInput {
        UsenetServerInput {
            id: None,
            name: "Primary".to_owned(),
            host: "news.example.test".to_owned(),
            port,
            tls: true,
            username: "reader".to_owned(),
            password: Some("hunter2".to_owned()),
            connections,
            priority: 10,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn an_invalid_port_or_connection_count_is_refused_before_writing() {
        let (_directory, database) = database().await;
        let store: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        for candidate in [input(0, 8), input(563, 0), input(563, 65)] {
            let error = save(&database, Arc::clone(&store), candidate, Timestamp::now())
                .await
                .expect_err("an invalid provider is refused");
            assert!(matches!(error, UsenetConfigError::Invalid(_)));
        }
        assert_eq!(database.usenet_servers().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_saved_password_reaches_the_engine_and_never_a_debug_string() {
        let (_directory, database) = database().await;
        let store: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
        save(
            &database,
            Arc::clone(&store),
            input(563, 8),
            Timestamp::now(),
        )
        .await
        .expect("a valid provider");
        let configs = server_configs(&database, Arc::clone(&store))
            .await
            .expect("the enabled provider");
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].nntp.host, "news.example.test");
        assert!(configs[0].nntp.password.is_some());
        assert!(!format!("{:?}", configs[0]).contains("hunter2"));
    }
}
