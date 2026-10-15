//! Fixtures the command tests share.
//!
//! One thing lives here, and it is here because four test modules need it and a fifth copy would be
//! four chances to leave one out: the credential store a started runtime is given.
//!
//! **A command test must never reach the developer's own keychain.** `RuntimeConfig` defaults to the
//! OS store, and on macOS the first read from a freshly built test binary raises an access prompt —
//! so a suite that takes the default waits on a person, which `AGENTS R14` forbids and which showed
//! up as a test taking over five minutes instead of milliseconds. This store answers deterministically:
//! nothing is stored, which is the state every installation starts in.

use peregrine_app::{Secret, SecretKey, SecretStore, SecretStoreError};

/// A credential store that holds nothing and accepts everything.
pub(crate) struct NoCredentials;

impl SecretStore for NoCredentials {
    fn get(&self, _key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
        Ok(None)
    }

    fn set(&self, _key: &SecretKey, _secret: &Secret) -> Result<(), SecretStoreError> {
        Ok(())
    }

    fn delete(&self, _key: &SecretKey) -> Result<(), SecretStoreError> {
        Ok(())
    }
}
