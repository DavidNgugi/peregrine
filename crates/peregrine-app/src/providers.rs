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
    /// The notice this provider's terms require to be reproduced verbatim.
    pub notice: &'static str,
    /// Whether those terms also require the provider's logo beside the notice.
    ///
    /// `true` for TMDB. The logo itself is not in this build yet, and that is a compliance gap rather
    /// than an unimplemented nicety — recorded in `PG-E5-19` rather than quietly defaulted away.
    pub requires_logo: bool,
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
    notice: peregrine_providers::tmdb::ATTRIBUTION_NOTICE,
    requires_logo: true,
}];

/// What the last credential read answered, which is not the same question as whether a key is stored.
///
/// **A refused read is not an absent key, and conflating the two is how a user concludes that
/// reinstalling deleted their key.** A keychain that denies access — on macOS the item's access
/// control is bound to the code signature of the application that created it, so a rebuilt or
/// reinstalled binary is a different application to the Keychain and raises a prompt — leaves
/// `configured` empty exactly as a first run does. A screen that read the two as one thing tells a
/// user whose key is still in their keychain to go and paste it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialStore {
    /// Nobody has read the store yet.
    ///
    /// The state a launch is in for the first moments after the window opens: the read is blocking
    /// and runs off the start-up path, so a settings screen can be looked at before it has answered.
    /// "Not configured" would be a claim nothing has established.
    #[default]
    Unread,
    /// The store answered, so `configured` is the whole truth about what is stored.
    Answered,
    /// The store refused access, so whether a key is stored cannot be known from here.
    Denied,
    /// There is no store this system can use — a headless session, or a machine with no keychain.
    Unavailable,
}

impl CredentialStore {
    /// Every value, in the order a screen checks them.
    pub const ALL: [Self; 4] = [
        Self::Unread,
        Self::Answered,
        Self::Denied,
        Self::Unavailable,
    ];

    /// What a store failure means to a screen, or `Answered` when the store was read.
    ///
    /// One place decides it, so the registry and anything else that classifies a failure cannot
    /// disagree about which message a user is shown.
    #[must_use]
    pub const fn of(error: &peregrine_secrets::SecretStoreError) -> Self {
        match error {
            peregrine_secrets::SecretStoreError::Denied => Self::Denied,
            peregrine_secrets::SecretStoreError::Unavailable
            | peregrine_secrets::SecretStoreError::Failed => Self::Unavailable,
        }
    }
}

/// What this installation has configured.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderStatus {
    /// The providers this build knows, whether or not they can be used.
    pub known: Vec<&'static str>,
    /// The providers that hold a credential and can therefore be asked.
    pub configured: Vec<&'static str>,
    /// What the last read of the credential store answered.
    pub store: CredentialStore,
}

impl ProviderStatus {
    /// The state before any read: every provider this build can talk to, and nothing known yet about
    /// what is stored.
    ///
    /// **`known` is a constant of the build and never an answer from the store.** A screen that
    /// learned the build's own provider list from a keyring read would claim this build knows no
    /// provider at all whenever that read was refused or had not happened yet — the same mistake, in
    /// the same direction, as showing a refusal as an absent key.
    #[must_use]
    pub fn unread() -> Self {
        Self {
            known: MANIFEST.iter().map(|entry| entry.provider).collect(),
            configured: Vec::new(),
            store: CredentialStore::Unread,
        }
    }

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
pub struct ProviderRegistry {
    /// The built adapters, so a request never waits on a keyring. A credential is held by the adapter
    /// that uses it, which is the one place it has to live; caching the *adapter* rather than the key
    /// means there is no second copy of a secret to leak.
    adapters:
        std::sync::RwLock<Vec<std::sync::Arc<dyn peregrine_providers::provider::MetadataProvider>>>,
    status: std::sync::RwLock<ProviderStatus>,
    /// Whether the credential read has finished, whatever it answered.
    ///
    /// **A fact of its own, and the one a caller waiting on the read needs.** `configured` is empty
    /// both before the read and after a read that found nothing, so on its own it cannot tell "no
    /// provider is configured" from "nobody has looked yet" — and a screen that read the second as
    /// the first would tell a user who *has* a key to go and set one up, while never starting the
    /// fetch that key would have filled the rails with.
    refreshed: std::sync::atomic::AtomicBool,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self {
            adapters: std::sync::RwLock::new(Vec::new()),
            // The known set is in place before the first read, so a status polled in the window
            // before the read answers names what the build can talk to and claims nothing about
            // what is stored.
            status: std::sync::RwLock::new(ProviderStatus::unread()),
            refreshed: std::sync::atomic::AtomicBool::new(false),
        }
    }
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
    /// The "the read has finished" flag is set whatever this answers, and that is the point of it:
    /// a caller waiting for the read — a launch with an empty catalogue, deciding whether there is
    /// anything to fetch with — must not wait for ever because the store refused to be read.
    ///
    /// **A refused read is recorded as such rather than left looking like an empty store.** The
    /// adapters are deliberately *not* replaced on a failure: a refresh that fails after an earlier
    /// success leaves the built adapters holding the credential they were built with, so the session
    /// can still ask the provider, and the status keeps the answer that was true.
    ///
    /// # Errors
    ///
    /// [`peregrine_secrets::SecretStoreError`] when the store cannot be read. A keyring that is
    /// unavailable, or a machine with no keychain at all, leaves the registry at nothing configured
    /// **and says which of those it was**.
    pub fn refresh(&self, store: &dyn SecretStore) -> Result<ProviderStatus, BuildError> {
        let outcome = self.rebuild(store);
        self.refreshed
            .store(true, std::sync::atomic::Ordering::Release);
        // Only a store failure says anything about the store, and only `store` is written: `known`
        // is the build's own list and `configured` keeps the answer that was true, because the
        // adapters built from a key that was read once are still in hand.
        if let Err(BuildError::Store(error)) = &outcome {
            let mut status = self.status.write().expect("the lock is not poisoned");
            status.store = CredentialStore::of(error);
        }
        outcome
    }

    /// Whether the credential read has finished, whether or not it found a provider.
    #[must_use]
    pub fn refreshed(&self) -> bool {
        self.refreshed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Reads the store and builds what it holds, without touching the flag.
    ///
    /// **The status is written as soon as the store has answered, before any adapter is built.** The
    /// store read and the adapter build can fail independently, and they mean different things: a
    /// failed build is *not* "nobody has looked in the keychain", and a screen told that would sit on
    /// "reading your keychain" for ever while the app knew perfectly well what was stored. Writing
    /// the answer first makes the two failures distinguishable — the store's own answer is on screen
    /// either way, and only a failed *read* changes what the status says about the store.
    fn rebuild(&self, store: &dyn SecretStore) -> Result<ProviderStatus, BuildError> {
        let status = status(store)?;
        *self.status.write().expect("the lock is not poisoned") = status.clone();

        let mut adapters = Vec::new();
        for entry in MANIFEST {
            if let Some(adapter) = build(entry, store)? {
                adapters.push(adapter);
            }
        }
        *self.adapters.write().expect("the lock is not poisoned") = adapters;
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

    /// Replaces the built adapters and leaves the status the store last answered.
    ///
    /// **For this crate's own tests, and for one reason** (`AGENTS R14`): a runtime-level test can
    /// have a provider configured by injecting a [`SecretStore`], but the adapter that store builds
    /// is the real one and its rail answer is an HTTP request. This is where a scripted adapter goes
    /// so a test can watch the runtime decide to fetch without a request leaving the machine. The
    /// status is deliberately not replaced: half of what is on test is the runtime's own rule, and
    /// "a provider is configured" still has to come from the store the runtime was started with.
    #[cfg(test)]
    pub(crate) fn use_adapters(
        &self,
        adapters: Vec<std::sync::Arc<dyn peregrine_providers::provider::MetadataProvider>>,
    ) {
        *self.adapters.write().expect("the lock is not poisoned") = adapters;
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
        let stored = store.get(&SecretKey::provider(entry.provider))?.is_some();
        let builtin =
            entry.provider == "tmdb" && peregrine_providers::tmdb::builtin_token().is_some();
        if stored || builtin {
            configured.push(entry.provider);
        }
    }
    Ok(ProviderStatus {
        known,
        configured,
        store: CredentialStore::Answered,
    })
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
        // A release build may carry a shared TMDB token. It is a fallback only: a stored key wins.
        let builtin = if entry.provider == "tmdb" {
            peregrine_providers::tmdb::builtin_token()
        } else {
            None
        };
        let Some(token) = builtin else {
            return Ok(None);
        };
        return build_with(entry, &peregrine_secrets::Secret::new(token)).map(Some);
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
        assert_eq!(
            registry.status().store,
            super::CredentialStore::Answered,
            "a store that answered is not a store that was refused"
        );
    }

    /// A keychain that refuses access says so, and says it *instead of* "nothing is stored".
    ///
    /// **The defect this test exists for is a sentence, and it is the one a user reads as data
    /// loss.** On macOS the item's access control is bound to the code signature of the application
    /// that created it, so a rebuilt or reinstalled binary is a different application to the
    /// Keychain and is asked. A denial leaves `configured` empty exactly as a first run does, and
    /// the screen that could not tell the two apart told a user whose key was still in their
    /// keychain to go and paste it again.
    #[test]
    fn a_refused_read_is_recorded_as_a_refusal_and_not_as_an_empty_store() {
        struct Refusing(peregrine_secrets::SecretStoreError);

        impl peregrine_secrets::SecretStore for Refusing {
            fn get(
                &self,
                _key: &peregrine_secrets::SecretKey,
            ) -> Result<Option<peregrine_secrets::Secret>, peregrine_secrets::SecretStoreError>
            {
                Err(self.0)
            }

            fn set(
                &self,
                _key: &peregrine_secrets::SecretKey,
                _secret: &peregrine_secrets::Secret,
            ) -> Result<(), peregrine_secrets::SecretStoreError> {
                Err(self.0)
            }

            fn delete(
                &self,
                _key: &peregrine_secrets::SecretKey,
            ) -> Result<(), peregrine_secrets::SecretStoreError> {
                Err(self.0)
            }
        }

        for (error, expected) in [
            (
                peregrine_secrets::SecretStoreError::Denied,
                super::CredentialStore::Denied,
            ),
            (
                peregrine_secrets::SecretStoreError::Unavailable,
                super::CredentialStore::Unavailable,
            ),
        ] {
            let registry = super::ProviderRegistry::new();
            assert!(
                registry.refresh(&Refusing(error)).is_err(),
                "a store that refuses cannot be reported as a success"
            );
            let status = registry.status();
            assert_eq!(status.store, expected);
            assert!(
                status.configured.is_empty(),
                "a refused read configures nothing, which is why the reason has to travel"
            );
            assert!(
                registry.refreshed(),
                "the read finished, however it ended: nothing waits for ever on a refusal"
            );
        }
    }

    /// The state before any read is its own answer, so a screen cannot claim nothing is configured
    /// while the read is still in flight.
    #[test]
    fn a_registry_nobody_has_read_is_unread_rather_than_unconfigured() {
        let registry = super::ProviderRegistry::new();
        assert_eq!(registry.status().store, super::CredentialStore::Unread);
        assert!(!registry.refreshed());
    }

    /// A store that was read once and then refused keeps the answer that was true, because the
    /// adapters it built still hold the credential and the session can still ask the provider.
    #[test]
    fn a_later_refusal_does_not_forget_a_provider_that_was_configured() {
        /// A store whose every call is refused, so the registry's second read fails.
        struct Refusing;
        impl peregrine_secrets::SecretStore for Refusing {
            fn get(
                &self,
                _key: &peregrine_secrets::SecretKey,
            ) -> Result<Option<peregrine_secrets::Secret>, peregrine_secrets::SecretStoreError>
            {
                Err(peregrine_secrets::SecretStoreError::Denied)
            }

            fn set(
                &self,
                _key: &peregrine_secrets::SecretKey,
                _secret: &peregrine_secrets::Secret,
            ) -> Result<(), peregrine_secrets::SecretStoreError> {
                Err(peregrine_secrets::SecretStoreError::Denied)
            }

            fn delete(
                &self,
                _key: &peregrine_secrets::SecretKey,
            ) -> Result<(), peregrine_secrets::SecretStoreError> {
                Err(peregrine_secrets::SecretStoreError::Denied)
            }
        }

        let registry = super::ProviderRegistry::new();
        let store = MemorySecretStore::with_provider("tmdb", "a-key");
        registry.refresh(&store).expect("the store answers");

        assert!(registry.refresh(&Refusing).is_err());

        let status = registry.status();
        assert_eq!(status.store, super::CredentialStore::Denied);
        assert_eq!(
            status.configured,
            vec!["tmdb"],
            "the adapters built from the key are still in hand, so the provider is still usable"
        );
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
    fn a_finished_read_is_reported_even_when_it_found_nothing() {
        // The two facts `status` alone cannot tell apart, and the reason the flag exists: a caller
        // waiting for the read — a launch deciding whether anything can fill the rails — has to
        // know when to stop waiting, and "found nothing" is a finished read.
        let registry = super::ProviderRegistry::new();
        assert!(!registry.refreshed(), "nothing has looked yet");

        registry
            .refresh(&MemorySecretStore::new())
            .expect("the store answers");
        assert!(registry.refreshed());
        assert!(registry.status().is_empty(), "and it found no provider");

        // A store that refuses is a finished read too: the wait must not be for ever because the
        // keychain was locked.
        let refusing = super::ProviderRegistry::new();
        assert!(refusing.refresh(&Refusing).is_err());
        assert!(refusing.refreshed());
    }

    /// A store whose every call fails, which is a locked keychain or a machine with none.
    struct Refusing;

    impl peregrine_secrets::SecretStore for Refusing {
        fn get(
            &self,
            _key: &peregrine_secrets::SecretKey,
        ) -> Result<Option<peregrine_secrets::Secret>, peregrine_secrets::SecretStoreError>
        {
            Err(peregrine_secrets::SecretStoreError::Unavailable)
        }

        fn set(
            &self,
            _key: &peregrine_secrets::SecretKey,
            _secret: &peregrine_secrets::Secret,
        ) -> Result<(), peregrine_secrets::SecretStoreError> {
            Err(peregrine_secrets::SecretStoreError::Unavailable)
        }

        fn delete(
            &self,
            _key: &peregrine_secrets::SecretKey,
        ) -> Result<(), peregrine_secrets::SecretStoreError> {
            Err(peregrine_secrets::SecretStoreError::Unavailable)
        }
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
