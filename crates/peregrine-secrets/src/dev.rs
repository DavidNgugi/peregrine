//! A file-backed secret store for debug builds.
//!
//! macOS ties a keychain item's access to the code signature of the binary that created it, and a
//! dev build is re-signed on every change. Each provider read therefore asks for the login password
//! again — three providers, three prompts, on every launch (SECURITY.md records the binding). This
//! store keeps the same values in a 0600 file under the app's data directory and reads the keyring
//! once for a value the file does not hold, so a developer is asked at most once and the prompts stop. A
//! release build never composes this store: `peregrine-ipc` installs it under `debug_assertions` only.
//!
//! **It is a debug-only trade, not a production backend.** The value is plaintext at rest, protected
//! by the file's mode and the user's own account, which is the same boundary the keychain has for this
//! user but without the OS prompt. The keyring item is deliberately left alone on a write or a delete,
//! so a release build still sees the credentials a developer entered there.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing::warn;

use crate::secret::Secret;
use crate::store::{KeyringStore, SecretKey, SecretStore, SecretStoreError};

/// The debug build's secret store: a 0600 JSON file, with the keyring as a one-time fallback.
pub struct DevFileStore {
    path: PathBuf,
    fallback: Arc<dyn SecretStore>,
    /// One writer at a time. The provider refresh and the settings screen both write, and a
    /// read-modify-write on one file would otherwise lose whichever write landed second.
    lock: Mutex<()>,
}

impl DevFileStore {
    /// The store at `path`, migrating values from the OS keyring on first read.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fallback: Arc::new(KeyringStore::new()),
            lock: Mutex::new(()),
        }
    }

    /// The same store with a caller-supplied fallback, which is what a test uses so it never
    /// reaches the developer's own keychain (AGENTS R14).
    #[must_use]
    pub fn with_fallback(path: impl Into<PathBuf>, fallback: Arc<dyn SecretStore>) -> Self {
        Self {
            path: path.into(),
            fallback,
            lock: Mutex::new(()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read(&self) -> BTreeMap<String, Option<String>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
            Err(_) => {
                warn!("the dev secret file could not be read; the keyring will be asked instead");
                return BTreeMap::new();
            }
        };
        serde_json::from_str(&text).unwrap_or_else(|_| {
            // A corrupt file is treated as empty so the keyring migration can repair it. The
            // value is not logged, and the parser message is not interpolated for the same
            // reason: it can quote the text it failed on.
            warn!("the dev secret file is not valid JSON; the keyring will be asked instead");
            BTreeMap::new()
        })
    }

    fn write(&self, map: &BTreeMap<String, Option<String>>) -> Result<(), SecretStoreError> {
        let parent = self.path.parent().ok_or(SecretStoreError::Failed)?;
        fs::create_dir_all(parent).map_err(|_| SecretStoreError::Failed)?;
        let json = serde_json::to_vec(map).map_err(|_| SecretStoreError::Failed)?;
        let temporary = self.path.with_extension("tmp");
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .map_err(|_| SecretStoreError::Failed)?;
            file.write_all(&json)
                .map_err(|_| SecretStoreError::Failed)?;
            file.sync_all().map_err(|_| SecretStoreError::Failed)?;
        }
        // A rename over an existing file is one atomic step on unix; Windows refuses the target if
        // it exists, so the old file goes first there.
        #[cfg(windows)]
        let _ = fs::remove_file(&self.path);
        fs::rename(&temporary, &self.path).map_err(|_| SecretStoreError::Failed)?;
        Ok(())
    }
}

impl SecretStore for DevFileStore {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
        let _guard = self.lock();
        let mut map = self.read();
        match map.get(key.as_str()) {
            Some(Some(value)) => return Ok(Some(Secret::new(value.clone()))),
            // A tombstone: the key was deleted in dev, and the keyring's copy must not be
            // migrated back in. The keyring keeps its value for a release build.
            Some(None) => return Ok(None),
            None => {}
        }
        // The one-time migration. A value the keyring already holds is copied here, so the prompt
        // above happens once rather than on every rebuild.
        let Some(secret) = self.fallback.get(key)? else {
            return Ok(None);
        };
        map.insert(
            key.as_str().to_owned(),
            Some(secret.expose_secret().to_owned()),
        );
        self.write(&map)?;
        Ok(Some(secret))
    }

    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretStoreError> {
        let _guard = self.lock();
        let mut map = self.read();
        map.insert(
            key.as_str().to_owned(),
            Some(secret.expose_secret().to_owned()),
        );
        self.write(&map)
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
        let _guard = self.lock();
        let mut map = self.read();
        // A tombstone rather than a removal: without it, the next read would fall back to the
        // keyring and migrate the value straight back.
        map.insert(key.as_str().to_owned(), None);
        self.write(&map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(Mutex<BTreeMap<String, String>>);

    impl SecretStore for Memory {
        fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
            Ok(self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(key.as_str())
                .map(Secret::new))
        }

        fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretStoreError> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.as_str().to_owned(), secret.expose_secret().to_owned());
            Ok(())
        }

        fn delete(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(key.as_str());
            Ok(())
        }
    }

    fn store() -> (tempfile::TempDir, DevFileStore) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let fallback = Arc::new(Memory::default());
        let store =
            DevFileStore::with_fallback(directory.path().join("secrets.dev.json"), fallback);
        (directory, store)
    }

    #[test]
    fn a_value_is_read_back_from_the_file_without_the_fallback() {
        let (_directory, store) = store();
        let key = SecretKey::provider("tmdb");
        store.set(&key, &Secret::new("first")).expect("write");
        assert_eq!(
            store
                .get(&key)
                .expect("read")
                .expect("present")
                .expose_secret(),
            "first"
        );
    }

    #[test]
    fn a_keyring_value_is_migrated_once_and_then_served_from_the_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let fallback = Arc::new(Memory::default());
        let key = SecretKey::provider("tmdb");
        fallback
            .set(&key, &Secret::new("from-keyring"))
            .expect("seed");
        let path = directory.path().join("secrets.dev.json");
        let store =
            DevFileStore::with_fallback(&path, Arc::clone(&fallback) as Arc<dyn SecretStore>);
        assert_eq!(
            store
                .get(&key)
                .expect("read")
                .expect("present")
                .expose_secret(),
            "from-keyring"
        );
        // The fallback is emptied; a second read must answer from the file.
        fallback.delete(&key).expect("clear");
        assert_eq!(
            store
                .get(&key)
                .expect("read")
                .expect("present")
                .expose_secret(),
            "from-keyring"
        );
        assert!(path.is_file());
    }

    #[test]
    fn deleting_removes_the_file_entry_and_leaves_the_keyring_alone() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let fallback = Arc::new(Memory::default());
        let key = SecretKey::provider("tmdb");
        fallback.set(&key, &Secret::new("release")).expect("seed");
        let store = DevFileStore::with_fallback(
            directory.path().join("secrets.dev.json"),
            Arc::clone(&fallback) as Arc<dyn SecretStore>,
        );
        store.set(&key, &Secret::new("dev")).expect("write");
        store.delete(&key).expect("delete");
        assert!(store.get(&key).expect("read").is_none());
        assert_eq!(
            fallback
                .get(&key)
                .expect("read")
                .expect("kept")
                .expose_secret(),
            "release"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("secrets.dev.json");
        let store = DevFileStore::with_fallback(&path, Arc::new(Memory::default()));
        store
            .set(&SecretKey::provider("tmdb"), &Secret::new("secret"))
            .expect("write");
        let mode = fs::metadata(&path).expect("the file").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
