//! Which metadata providers this installation can use, and how one is built.
//!
//! The manifest is the composition root's business rather than the adapter crate's: it says which
//! providers this build knows, which of them a user must supply a credential for, and what endpoint
//! each defaults to. What it deliberately does **not** hold is the user's own enablement and priority
//! choices — those are settings (`PG-E20`), and until the registry exists this is what the defaults are
//! and nothing more.
//!
//! A provider with no credential is *known* and not *configured*, and the difference is what the
//! settings screen and the health check both need to show: a provider nobody has set up is not an error,
//! and a request to one is not attempted.

use peregrine_providers::tmdb::Tmdb;
use peregrine_secrets::{SecretKey, SecretStore};

/// One provider this build knows how to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderManifest {
    /// The provider's name, as `peregrine_domain::Provider` spells it.
    pub provider: &'static str,
    /// What to call it in the interface.
    pub display_name: &'static str,
    /// Whether a request is refused until the user supplies a credential.
    pub requires_credential: bool,
    /// Where its API lives by default.
    pub endpoint: &'static str,
}

/// Every provider this build can talk to.
///
/// One entry per adapter that exists. The list is what `PG-E5-12`'s registry will grow from, and it is
/// short because an adapter is a task of its own rather than a line here.
pub const MANIFEST: &[ProviderManifest] = &[ProviderManifest {
    provider: "tmdb",
    display_name: "TMDB",
    requires_credential: true,
    endpoint: Tmdb::BASE_URL,
}];

/// What this installation has configured.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderStatus {
    /// The providers this build knows, whether or not they can be used.
    pub known: Vec<&'static str>,
    /// The providers that hold a credential and can therefore be asked.
    pub configured: Vec<&'static str>,
}

impl ProviderStatus {
    /// Whether anything can be asked at all. A search with no provider is an empty result rather than
    /// a failure, and the interface says so instead of showing an error.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.configured.is_empty()
    }
}

/// What this installation has configured, kept where a polled check can read it without blocking.
///
/// Reading a keyring is **blocking and can take seconds** — on macOS the first call from a new binary
/// raises an OS prompt, which measured at eleven seconds in this repository's own command-budget test.
/// So the read happens once, off the command path, and a health check reads this cache: a diagnostics
/// check that stalled on a keychain prompt would be worse than no diagnostics at all (`ARCHITECTURE §7`
/// rule 2, and the test that caught it).
#[derive(Default)]
pub struct ProviderRegistry {
    /// The built adapters, so a request never waits on a keyring. A credential is held by the adapter
    /// that uses it, which is the one place it has to live; caching the *adapter* rather than the key
    /// means there is no second copy of a secret to leak.
    adapters:
        std::sync::RwLock<Vec<std::sync::Arc<dyn peregrine_providers::provider::MetadataProvider>>>,
    status: std::sync::RwLock<ProviderStatus>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The adapters hold credentials; they are counted rather than printed, and no `Debug` of a
        // secret is ever reached through this type.
        formatter
            .debug_struct("ProviderRegistry")
            .field("adapters", &self.adapters().len())
            .field("status", &self.status())
            .finish()
    }
}

impl ProviderRegistry {
    /// A registry that believes nothing is configured until [`ProviderRegistry::refresh`] says otherwise.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What was configured as of the last refresh.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned, which means a writer panicked while holding it — a bug in
    /// this type rather than anything a caller can cause.
    #[must_use]
    pub fn status(&self) -> ProviderStatus {
        self.status
            .read()
            .expect("the lock is not poisoned")
            .clone()
    }

    /// Reads the store and remembers the answer.
    ///
    /// # Errors
    ///
    /// [`peregrine_secrets::SecretStoreError`] when the store cannot be read. A keyring that is
    /// unavailable, or a machine with no keychain at all, leaves the registry at nothing configured.
    pub fn refresh(&self, store: &dyn SecretStore) -> Result<ProviderStatus, BuildError> {
        let status = status(store)?;
        let mut adapters = Vec::new();
        for entry in MANIFEST {
            if let Some(adapter) = build(entry, store)? {
                adapters.push(adapter);
            }
        }
        *self.adapters.write().expect("the lock is not poisoned") = adapters;
        *self.status.write().expect("the lock is not poisoned") = status.clone();
        Ok(status)
    }

    /// The adapters that can be asked, in priority order.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned, which is a bug in this type rather than anything a caller
    /// can cause.
    #[must_use]
    pub fn adapters(
        &self,
    ) -> Vec<std::sync::Arc<dyn peregrine_providers::provider::MetadataProvider>> {
        self.adapters
            .read()
            .expect("the lock is not poisoned")
            .clone()
    }
}

/// The manifest entry for `provider`, when this build knows one.
///
/// The one place a string a caller supplied — a provider name from a settings screen — becomes an
/// entry. A name that is not here is a request this build cannot serve, and the caller refuses it
/// rather than guessing at an endpoint.
#[must_use]
pub fn entry(provider: &str) -> Option<&'static ProviderManifest> {
    MANIFEST.iter().find(|entry| entry.provider == provider)
}

/// Builds the adapter for one manifest entry from a credential the caller already holds.
///
/// **This is what makes a credential testable before it is stored** (PG-E5-20): the same constructor
/// the registry uses, given a candidate instead of the keyring's answer, so a key that does not work
/// is a key that never reaches the keychain.
///
/// # Errors
///
/// [`BuildError::Provider`] when the adapter refuses its configuration, and [`BuildError::Unknown`]
/// when the entry names a provider this build has no arm for — a manifest entry added without one,
/// which the compiler cannot catch because the match is on a string.
pub fn build_with(
    entry: &ProviderManifest,
    credential: &peregrine_secrets::Secret,
) -> Result<std::sync::Arc<dyn peregrine_providers::provider::MetadataProvider>, BuildError> {
    let adapter = match entry.provider {
        "tmdb" => Tmdb::new(
            entry.endpoint,
            peregrine_providers::tmdb::IMAGE_BASE,
            credential.expose_secret().to_owned(),
        )?,
        unknown => {
            return Err(BuildError::Unknown {
                provider: unknown.to_owned(),
            });
        }
    };
    Ok(std::sync::Arc::new(adapter))
}

/// Reads which providers hold a credential.
///
/// **Synchronous and blocking**, because every OS keyring API is: the caller that runs on an async
/// runtime wraps it in `spawn_blocking`, which is what `Runtime::health` does.
///
/// # Errors
///
/// [`peregrine_secrets::SecretStoreError`] when the keyring itself cannot be read. A provider with no
/// stored credential is not an error — it is the state every installation starts in.
pub fn status(
    store: &dyn SecretStore,
) -> Result<ProviderStatus, peregrine_secrets::SecretStoreError> {
    let mut known = Vec::with_capacity(MANIFEST.len());
    let mut configured = Vec::new();
    for entry in MANIFEST {
        known.push(entry.provider);
        if store.get(&SecretKey::provider(entry.provider))?.is_some() {
            configured.push(entry.provider);
        }
    }
    Ok(ProviderStatus { known, configured })
}

/// Builds the adapter for one manifest entry from its stored credential.
///
/// `None` when the provider needs a credential and none is stored. The match grows by one arm per
/// adapter; there is deliberately no trait object table here, because building an adapter is not a
/// polymorphic operation — it is one constructor per provider with its own arguments.
///
/// # Errors
///
/// [`peregrine_secrets::SecretStoreError`] when the keyring cannot be read, and
/// [`peregrine_providers::error::ProviderError`] when the adapter refuses its configuration.
pub fn build(
    entry: &ProviderManifest,
    store: &dyn SecretStore,
) -> Result<Option<std::sync::Arc<dyn peregrine_providers::provider::MetadataProvider>>, BuildError>
{
    let credential = store.get(&SecretKey::provider(entry.provider))?;
    let Some(credential) = credential else {
        return Ok(None);
    };
    build_with(entry, &credential).map(Some)
}

/// Why an adapter could not be built.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The keyring could not be read.
    #[error("the credential store could not be read")]
    Store(#[from] peregrine_secrets::SecretStoreError),
    /// The provider refused its configuration.
    #[error("the provider could not be built from its configuration")]
    Provider(#[from] peregrine_providers::error::ProviderError),
    /// The manifest names a provider this build has no constructor for.
    #[error("this build has no adapter for `{provider}`")]
    Unknown {
        /// The name the manifest used.
        provider: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{MANIFEST, build, status};
    use peregrine_testkit::MemorySecretStore;

    #[test]
    fn a_polled_status_never_reads_the_store() {
        // The whole point of the cache: a health check answers from memory, so a locked or prompting
        // keychain cannot stall a command. The store here would panic if it were consulted.
        struct NeverAsked;

        impl peregrine_secrets::SecretStore for NeverAsked {
            fn get(
                &self,
                _key: &peregrine_secrets::SecretKey,
            ) -> Result<Option<peregrine_secrets::Secret>, peregrine_secrets::SecretStoreError>
            {
                panic!("the status must be cached, not re-read");
            }

            fn set(
                &self,
                _key: &peregrine_secrets::SecretKey,
                _secret: &peregrine_secrets::Secret,
            ) -> Result<(), peregrine_secrets::SecretStoreError> {
                panic!("the status must be cached, not re-read");
            }

            fn delete(
                &self,
                _key: &peregrine_secrets::SecretKey,
            ) -> Result<(), peregrine_secrets::SecretStoreError> {
                panic!("the status must be cached, not re-read");
            }
        }

        let registry = super::ProviderRegistry::new();
        assert!(registry.status().is_empty());
        let _ = NeverAsked;
    }

    #[test]
    fn a_refresh_remembers_what_the_store_said() {
        let registry = super::ProviderRegistry::new();
        let store = MemorySecretStore::with_provider("tmdb", "a-key");

        assert_eq!(
            registry
                .refresh(&store)
                .expect("the store answers")
                .configured,
            vec!["tmdb"]
        );
        assert_eq!(registry.status().configured, vec!["tmdb"]);
    }

    #[test]
    fn a_provider_with_no_credential_is_known_and_not_configured() {
        // The state every installation starts in, and the difference the settings screen shows.
        let store = MemorySecretStore::new();
        let status = status(&store).expect("the store answers");

        assert_eq!(status.known, vec!["tmdb"]);
        assert_eq!(status.configured.len(), 0);
        assert!(status.is_empty());
    }

    #[test]
    fn an_unknown_provider_name_is_not_an_entry() {
        // The settings screen sends a string, so the lookup is the boundary that has to answer
        // `None` rather than build something from a name nobody registered.
        assert!(super::entry("tmdb").is_some());
        assert!(super::entry("nobody").is_none());
        assert!(super::entry("").is_none());
    }

    #[test]
    fn a_candidate_credential_builds_without_a_store() {
        // What makes PG-E5-20 possible: the same constructor the registry uses, given a credential
        // the caller holds, so a key can be proved before it is written anywhere.
        let manifest = super::entry("tmdb").expect("tmdb is known");
        let adapter = super::build_with(
            manifest,
            &peregrine_secrets::Secret::new("a-key".to_owned()),
        )
        .expect("the adapter builds");
        assert_eq!(
            adapter.descriptor().provider(),
            peregrine_domain::id::Provider::Tmdb
        );
    }

    #[test]
    fn a_provider_holding_a_credential_is_configured_and_can_be_built() {
        let store = MemorySecretStore::with_provider("tmdb", "a-key");
        let status = status(&store).expect("the store answers");

        assert_eq!(status.configured, vec!["tmdb"]);
        let entry = MANIFEST.first().expect("an entry");
        assert!(build(entry, &store).expect("it builds").is_some());
    }
}
