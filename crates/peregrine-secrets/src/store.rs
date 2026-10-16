//! Where secrets live: the OS keyring, behind a port.
//!
//! The keyring is an external system, so it sits behind a trait like every other one (AGENTS R8) and
//! the app calls the trait. That is not ceremony here — a test cannot reach a real keyring without
//! touching the developer's own login keychain, and `AGENTS R14` says a test does neither.
//!
//! **Nothing in this module logs, formats, or returns a secret.** `Secret` already refuses to print
//! itself, the error type carries no detail, and a key *name* is not a secret: it names which provider
//! a value belongs to, which is not sensitive and is what an error can safely say.

use crate::secret::Secret;

/// The name one provider's credential is stored under.
///
/// A newtype rather than a `String` because the name is a contract with every previous release: change
/// the shape and every stored key becomes unreachable, silently, on users' machines. One function
/// builds it, so there is one place to read that shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretKey(String);

impl SecretKey {
    /// The service name every entry is stored under, which is what a user sees in their keychain.
    pub const SERVICE: &'static str = "Peregrine";

    /// The key for one provider's API credential.
    #[must_use]
    pub fn provider(provider: &str) -> Self {
        Self(format!("provider.{provider}"))
    }

    /// The key for one provider's endpoint, such as a self-hosted indexer's base URL.
    ///
    /// A separate entry rather than part of a credential because a URL is configuration and a key is
    /// a secret: a screen may show the first and must never show the second.
    #[must_use]
    pub fn endpoint(provider: &str) -> Self {
        Self(format!("endpoint.{provider}"))
    }

    /// The name, as the store sees it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a store could not answer.
///
/// No variant carries a message or a source: a keyring error can quote the entry it failed on, and the
/// one rule this module exists to keep is that a secret never reaches a log, a diagnostic bundle, or a
/// user-facing message. The variant says what happened and the caller decides what to tell the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretStoreError {
    /// No keyring is available — a headless session, a container, or a locked login keychain.
    #[error("no keyring is available on this system")]
    Unavailable,
    /// The user refused access, or the system denied it.
    #[error("access to the keyring was denied")]
    Denied,
    /// The store failed for a reason it did not describe.
    #[error("the keyring could not be read or written")]
    Failed,
}

/// Where a credential is kept.
///
/// Synchronous, because every OS keyring API is: the caller that must not block runs it in
/// `spawn_blocking`, which is the caller's decision to make and not this trait's.
pub trait SecretStore: Send + Sync {
    /// The secret stored under `key`, or `None` when nothing is.
    ///
    /// # Errors
    ///
    /// [`SecretStoreError`] when the store cannot be read. An absent entry is `Ok(None)`, because
    /// "nobody has configured this yet" is the normal first-run state and not an error to report.
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError>;

    /// Stores `secret` under `key`, replacing anything already there.
    ///
    /// # Errors
    ///
    /// [`SecretStoreError`] when the store cannot be written.
    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretStoreError>;

    /// Removes whatever is stored under `key`.
    ///
    /// # Errors
    ///
    /// [`SecretStoreError`] when the store cannot be written. Removing nothing is `Ok(())`: a
    /// disconnect that is already disconnected has done what it was asked.
    fn delete(&self, key: &SecretKey) -> Result<(), SecretStoreError>;
}

/// The OS keyring: macOS Keychain, Windows Credential Manager, or the Secret Service on Linux.
///
/// Built with `keyring`'s v1 compatibility API over the three native store features. The crate's own
/// documentation suggests linking `keyring-core` and the store crates directly for an application, and
/// that is the direction to move if Linux's `zbus` dependency ever costs more than it earns — recorded
/// here because it is a real trade and not an oversight.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyringStore {
    /// A store under [`SecretKey::SERVICE`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: SecretKey::SERVICE.to_owned(),
        }
    }

    /// The entry for one key.
    fn entry(&self, key: &SecretKey) -> Result<keyring::v1::Entry, SecretStoreError> {
        keyring::v1::Entry::new(&self.service, key.as_str()).map_err(|error| classify(&error))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(classify(&error)),
        }
    }

    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretStoreError> {
        self.entry(key)?
            .set_password(secret.expose_secret())
            .map_err(|error| classify(&error))
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(classify(&error)),
        }
    }
}

/// Which of this module's three answers an OS error is.
fn classify(error: &keyring::Error) -> SecretStoreError {
    match error {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            SecretStoreError::Unavailable
        }
        keyring::Error::NoEntry => SecretStoreError::Failed,
        _ => SecretStoreError::Denied,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{SecretKey, SecretStore, SecretStoreError};
    use crate::secret::Secret;

    /// A store that keeps secrets in memory, to test the contract this module states. Test-only: a real
    /// one belongs to whoever needs it, and `peregrine-testkit` is where a shared fake goes.
    #[derive(Default)]
    struct MemoryStore {
        values: Mutex<HashMap<String, String>>,
    }

    impl SecretStore for MemoryStore {
        fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
            let values = self.values.lock().expect("the lock is not poisoned");
            Ok(values.get(key.as_str()).map(Secret::new))
        }

        fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretStoreError> {
            let mut values = self.values.lock().expect("the lock is not poisoned");
            values.insert(key.as_str().to_owned(), secret.expose_secret().to_owned());
            Ok(())
        }

        fn delete(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
            let mut values = self.values.lock().expect("the lock is not poisoned");
            values.remove(key.as_str());
            Ok(())
        }
    }

    #[test]
    fn a_key_is_stable_and_names_only_the_provider() {
        // The name is a contract with every previous release: changing it makes stored keys unreachable
        // on users' machines, silently. This test is the shape, written down.
        assert_eq!(SecretKey::provider("tmdb").as_str(), "provider.tmdb");
        assert_eq!(SecretKey::SERVICE, "Peregrine");
    }

    #[test]
    fn the_store_contract_is_absent_not_error_missing_and_absent_after_delete() {
        // What any implementation owes its callers, and what the keyring adapter maps `NoEntry` to.
        let store = MemoryStore::default();
        let key = SecretKey::provider("tmdb");

        assert!(store.get(&key).expect("a read").is_none());
        store
            .set(&key, &Secret::new("a-key".to_owned()))
            .expect("a write");
        assert_eq!(
            store
                .get(&key)
                .expect("a read")
                .expect("a value")
                .expose_secret(),
            "a-key"
        );
        store.delete(&key).expect("a delete");
        assert!(store.get(&key).expect("a read").is_none());
        store.delete(&key).expect("deleting nothing is fine");
    }
}
