//! A secret store that keeps everything in memory.

use std::collections::HashMap;
use std::sync::Mutex;

use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};

/// A [`SecretStore`] that keeps secrets in memory and loses them when it is dropped.
///
/// It records whether a key *was* set, so a test can assert that a provider was configured without the
/// value ever being read back — which is how a test asserts about a secret without handling one.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A store holding one provider's credential, for the common case.
    #[must_use]
    pub fn with_provider(provider: &str, secret: &str) -> Self {
        let store = Self::new();
        store
            .values
            .lock()
            .expect("the lock is not poisoned")
            .insert(
                SecretKey::provider(provider).as_str().to_owned(),
                secret.to_owned(),
            );
        store
    }
}

impl SecretStore for MemorySecretStore {
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

#[cfg(test)]
mod tests {
    use super::MemorySecretStore;
    use peregrine_secrets::{SecretKey, SecretStore};

    #[test]
    fn a_store_holding_a_provider_credential_answers_for_that_provider_only() {
        let store = MemorySecretStore::with_provider("tmdb", "a-key");

        assert!(
            store
                .get(&SecretKey::provider("tmdb"))
                .expect("a read")
                .is_some()
        );
        assert!(
            store
                .get(&SecretKey::provider("tvdb"))
                .expect("a read")
                .is_none()
        );
    }
}
