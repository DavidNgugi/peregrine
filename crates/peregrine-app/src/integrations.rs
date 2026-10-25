//! The integrations this build knows, what each one can do, and what this installation chose.
//!
//! **The manifest is the composition root's, not an adapter's.** It is one table the settings
//! screen renders, the marker resolver orders, and the credential job probes; a second table in a
//! screen would be a second answer to "what can be configured" (AGENTS R7).
//!
//! The rules that matter are visible here rather than at a call site:
//!
//! - **`TheIntroDB` and `AniSkip` need no key for reads.** Their `apiKey` field is optional (or
//!   absent), and `configured` is true with nothing stored. A settings screen that showed
//!   "needs attention" for them would be wrong about the product.
//! - **A key is never read back.** The registry caches only whether a field is present, and the
//!   status DTO carries no value, no length, and no prefix.
//! - **A keychain refusal is its own state.** A denied read is carried as `CredentialStore::Denied`
//!   and is never collapsed into "not configured" (the defect PG-E5-12 records).
//! - **Enablement and priority are installation-level.** Playback preferences are per profile and
//!   live in `playback_preference`; these do not.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use peregrine_domain::{
    IntegrationCapability, IntegrationCategory, IntegrationId, IntegrationOutcome,
    IntegrationPreference,
};
use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};

use crate::providers::CredentialStore;

/// The namespace a credential field is stored under.
///
/// TMDB is the one integration whose key predates this table and lives under
/// `provider.tmdb`; every new one is stored under `integration.<id>.<field>`. Encoding the
/// difference here is what lets the registry read an existing installation's key instead of
/// asking for it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialNamespace {
    /// An existing provider credential, stored under `provider.<name>`.
    Provider(&'static str),
    /// An integration field, stored under `integration.<id>.<field>`.
    Integration,
}

/// One value an integration asks a person for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialField {
    /// The field's stable name, which is part of the keyring account.
    pub name: &'static str,
    /// What the settings screen calls it.
    pub label: &'static str,
    /// Whether the value is a password field. No field is ever a text field today, but the flag
    /// keeps a future non-secret field from being masked by accident.
    pub secret: bool,
    /// Whether the integration can be used without it.
    pub required: bool,
    /// Where the value is stored.
    pub namespace: CredentialNamespace,
}

/// How a bounded probe of one integration works.
///
/// A closed enum rather than a URL or a callback: the probe is part of the manifest, and the job
/// that runs it is the only place that turns a probe into a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Nothing to probe: the source is local and needs no service.
    None,
    /// The metadata adapter's own `check`.
    Tmdb,
    /// A public `TheIntroDB` lookup for a documented example episode.
    IntroDb,
    /// A public `AniSkip` lookup for a documented anime episode.
    AniSkip,
}

/// One integration this build can configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrationManifest {
    /// The stable id, which is the settings row and the keyring namespace.
    pub id: &'static str,
    /// What the settings screen calls it.
    pub display_name: &'static str,
    /// One sentence about what it does and what it needs.
    pub description: &'static str,
    /// Where its documentation lives.
    pub docs: &'static str,
    /// Where its card sits.
    pub category: IntegrationCategory,
    /// What it can be asked for.
    pub capabilities: &'static [IntegrationCapability],
    /// The values a person may enter. Empty for a source with no credentials.
    pub credential_fields: &'static [CredentialField],
    /// Whether it accepts contributed timestamps.
    pub submission_support: bool,
    /// Whether the source is available without a person enabling it.
    ///
    /// True only for local chapters: a network source that enabled itself would be an egress
    /// nobody asked for (AGENTS R23).
    pub always_on: bool,
    /// Its order within its category before a person changes it.
    pub default_priority: u32,
    /// How to test it.
    pub probe: Probe,
}

/// The integrations this build knows.
///
/// **One entry per adapter that exists.** `OpenSubtitles`, `Subdl`, `IntroHater`, and Trakt are not here
/// yet, and adding a card for a service nothing can call would be a promise the build cannot keep.
pub const MANIFEST: &[IntegrationManifest] = &[
    IntegrationManifest {
        id: "chapters",
        display_name: "Local chapters",
        description: "The file's own chapter names, read with ffprobe. Always available and more trustworthy than any database.",
        docs: "https://matroska.org/technical/elements.html",
        category: IntegrationCategory::SkipMarkers,
        capabilities: &[IntegrationCapability::Markers],
        credential_fields: &[],
        submission_support: false,
        always_on: true,
        default_priority: 0,
        probe: Probe::None,
    },
    IntegrationManifest {
        id: "theintrodb",
        display_name: "TheIntroDB",
        description: "Community intro, recap, credits, and preview timestamps. Reads need no key; a key is only for submitting timestamps.",
        docs: "https://theintrodb.org",
        category: IntegrationCategory::SkipMarkers,
        capabilities: &[IntegrationCapability::Markers],
        credential_fields: &[CredentialField {
            name: "apiKey",
            label: "User API key (optional, for submissions)",
            secret: true,
            required: false,
            namespace: CredentialNamespace::Integration,
        }],
        submission_support: true,
        always_on: false,
        default_priority: 10,
        probe: Probe::IntroDb,
    },
    IntegrationManifest {
        id: "aniskip",
        display_name: "AniSkip",
        description: "Anime intro and ending timestamps. Needs no key, and is asked only for a series with a MyAnimeList id.",
        docs: "https://aniskip.com",
        category: IntegrationCategory::SkipMarkers,
        capabilities: &[IntegrationCapability::Markers],
        credential_fields: &[],
        submission_support: false,
        always_on: false,
        default_priority: 20,
        probe: Probe::AniSkip,
    },
    IntegrationManifest {
        id: "tmdb",
        display_name: "TMDB",
        description: "Title, season, episode, and artwork metadata. A key is required.",
        docs: "https://developer.themoviedb.org/docs",
        category: IntegrationCategory::Metadata,
        capabilities: &[IntegrationCapability::Metadata],
        credential_fields: &[CredentialField {
            name: "apiKey",
            label: "API key",
            secret: true,
            required: true,
            namespace: CredentialNamespace::Provider("tmdb"),
        }],
        submission_support: false,
        always_on: false,
        default_priority: 0,
        probe: Probe::Tmdb,
    },
];

/// The manifest entry with this id, if this build knows it.
#[must_use]
pub fn find(id: &str) -> Option<&'static IntegrationManifest> {
    MANIFEST.iter().find(|entry| entry.id == id)
}

impl IntegrationManifest {
    /// The id as the validated type.
    ///
    /// # Panics
    ///
    /// Panics when the manifest holds an id this build's validator refuses. That is a programming
    /// error in the table above, not something a caller can cause, and a test checks every entry.
    #[must_use]
    pub fn id(&self) -> IntegrationId {
        IntegrationId::new(self.id).expect("every manifest id is valid")
    }

    /// Whether a request is refused until every required field is stored.
    #[must_use]
    pub fn requires_key(&self) -> bool {
        self.credential_fields.iter().any(|field| field.required)
    }

    /// Whether the integration accepts a key but works without one.
    #[must_use]
    pub fn key_optional(&self) -> bool {
        !self.requires_key() && !self.credential_fields.is_empty()
    }

    /// The keyring account one field is stored under.
    #[must_use]
    pub fn secret_key(&self, field: &CredentialField) -> SecretKey {
        match field.namespace {
            CredentialNamespace::Provider(provider) => SecretKey::provider(provider),
            CredentialNamespace::Integration => SecretKey::integration(self.id, field.name),
        }
    }

    /// The state a never-configured installation starts in.
    #[must_use]
    pub fn default_preference(&self) -> IntegrationPreference {
        if self.always_on {
            IntegrationPreference::always_on(self.id(), self.default_priority)
        } else {
            IntegrationPreference::default_for(self.id(), self.default_priority)
        }
    }
}

/// One integration's facts, merged from the manifest, the stored choice, and the keyring cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationStatus {
    /// The manifest entry.
    pub manifest: &'static IntegrationManifest,
    /// The stored or default choice.
    pub preference: IntegrationPreference,
    /// Whether every required field is present.
    pub configured: bool,
    /// What the last credential read answered.
    pub store: CredentialStore,
    /// What the last probe answered, when one has run.
    pub last_outcome: Option<IntegrationOutcome>,
    /// When the last probe ran, in Unix milliseconds.
    pub last_test_at: Option<i64>,
    /// The stable code the last failed probe recorded.
    pub last_error_code: Option<String>,
}

/// A candidate credential set waiting for the job that will prove it.
///
/// **The candidate never enters the journal.** A job row is a database row, and a database row is
/// backed up and read by diagnostics; a key in `payload_json` would be a key in all three
/// (SECURITY.md T13). The job carries a one-use token and takes the values from this map.
pub struct PendingCredentials {
    /// The integration the values belong to.
    pub integration: IntegrationId,
    /// One value per field, in manifest order.
    pub values: Vec<(String, Secret)>,
}

/// What this installation has configured, and the keyring facts the settings screen reads.
pub struct IntegrationRegistry {
    stored: RwLock<Vec<StoredChoice>>,
    present: RwLock<HashSet<String>>,
    store: RwLock<CredentialStore>,
    pending: Mutex<HashMap<u64, PendingCredentials>>,
    next_token: AtomicU64,
}

/// The stored half of one integration, in the shape the registry merges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChoice {
    /// The stored preference.
    pub preference: IntegrationPreference,
    /// When the last probe ran.
    pub last_test_at: Option<i64>,
    /// What it answered.
    pub last_outcome: Option<IntegrationOutcome>,
    /// The stable code, when it failed.
    pub last_error_code: Option<String>,
}

impl IntegrationRegistry {
    /// Builds a registry from the rows the database holds. Absent entries use the manifest default.
    #[must_use]
    pub fn new(stored: Vec<peregrine_storage::StoredIntegration>) -> Self {
        let choices: Vec<StoredChoice> = stored
            .into_iter()
            .map(|row| StoredChoice {
                preference: row.preference,
                last_test_at: row.last_test_at,
                last_outcome: row.last_outcome,
                last_error_code: row.last_error_code,
            })
            .collect();
        Self {
            stored: RwLock::new(choices),
            present: RwLock::new(HashSet::new()),
            store: RwLock::new(CredentialStore::Unread),
            pending: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(1),
        }
    }

    /// The merged status of every integration, in manifest order.
    #[must_use]
    pub fn status(&self) -> Vec<IntegrationStatus> {
        let stored = self.stored.read().expect("the lock is not poisoned");
        let present = self.present.read().expect("the lock is not poisoned");
        let store = *self.store.read().expect("the lock is not poisoned");
        MANIFEST
            .iter()
            .map(|manifest| {
                let choice = stored
                    .iter()
                    .find(|row| row.preference.id.as_str() == manifest.id);
                let preference = choice.map_or_else(
                    || manifest.default_preference(),
                    |row| row.preference.clone(),
                );
                let configured = manifest.credential_fields.iter().all(|field| {
                    if !field.required {
                        return true;
                    }
                    let key = manifest.secret_key(field);
                    present.contains(key.as_str())
                });
                IntegrationStatus {
                    manifest,
                    preference,
                    configured,
                    store,
                    last_outcome: choice.and_then(|row| row.last_outcome),
                    last_test_at: choice.and_then(|row| row.last_test_at),
                    last_error_code: choice.and_then(|row| row.last_error_code.clone()),
                }
            })
            .collect()
    }

    /// The merged choice for one integration.
    #[must_use]
    pub fn preference(&self, manifest: &IntegrationManifest) -> IntegrationPreference {
        self.stored
            .read()
            .expect("the lock is not poisoned")
            .iter()
            .find(|row| row.preference.id.as_str() == manifest.id)
            .map_or_else(
                || manifest.default_preference(),
                |row| row.preference.clone(),
            )
    }

    /// Which integrations are enabled and can be asked, in the manifest's order.
    #[must_use]
    pub fn enabled(&self) -> Vec<&'static IntegrationManifest> {
        let stored = self.stored.read().expect("the lock is not poisoned");
        let mut enabled: Vec<&'static IntegrationManifest> = MANIFEST
            .iter()
            .filter(|manifest| {
                stored
                    .iter()
                    .find(|row| row.preference.id.as_str() == manifest.id)
                    .map_or(manifest.always_on, |row| row.preference.enabled)
            })
            .collect();
        enabled.sort_by_key(|manifest| {
            let preference = stored
                .iter()
                .find(|row| row.preference.id.as_str() == manifest.id)
                .map_or_else(
                    || manifest.default_preference(),
                    |row| row.preference.clone(),
                );
            (preference.priority, manifest.id)
        });
        enabled
    }

    /// Replaces one stored choice in the cache after the database accepted it.
    pub fn remember(&self, preference: IntegrationPreference) {
        let mut stored = self.stored.write().expect("the lock is not poisoned");
        match stored
            .iter_mut()
            .find(|row| row.preference.id == preference.id)
        {
            Some(row) => row.preference = preference,
            None => stored.push(StoredChoice {
                preference,
                last_test_at: None,
                last_outcome: None,
                last_error_code: None,
            }),
        }
    }

    /// Records what a probe answered, after the database accepted it.
    pub fn remember_outcome(
        &self,
        id: &IntegrationId,
        outcome: IntegrationOutcome,
        error_code: Option<&str>,
        at: i64,
    ) {
        let mut stored = self.stored.write().expect("the lock is not poisoned");
        if let Some(row) = stored.iter_mut().find(|row| row.preference.id == *id) {
            row.last_outcome = Some(outcome);
            row.last_test_at = Some(at);
            row.last_error_code = error_code.map(str::to_owned);
        } else {
            let Some(manifest) = find(id.as_str()) else {
                return;
            };
            stored.push(StoredChoice {
                preference: manifest.default_preference(),
                last_test_at: Some(at),
                last_outcome: Some(outcome),
                last_error_code: error_code.map(str::to_owned),
            });
        }
    }

    /// The last credential read.
    #[must_use]
    pub fn store_state(&self) -> CredentialStore {
        *self.store.read().expect("the lock is not poisoned")
    }

    /// Reads every manifest field from the store, recording presence only.
    ///
    /// Runs on a blocking thread by the caller: a keyring read is synchronous and can raise an OS
    /// prompt (PG-E5-12). A refusal is recorded as `Denied` or `Unavailable`, never as an empty
    /// store, and a later refusal does not forget a presence already known.
    pub fn refresh(&self, store: &dyn SecretStore) {
        let mut present = HashSet::new();
        let mut state = CredentialStore::Answered;
        for manifest in MANIFEST {
            for field in manifest.credential_fields {
                let key = manifest.secret_key(field);
                match store.get(&key) {
                    Ok(Some(_)) => {
                        present.insert(key.as_str().to_owned());
                    }
                    Ok(None) => {}
                    Err(error) => {
                        state = CredentialStore::of(&error);
                        // A refusal ends the read: continuing would either prompt again or
                        // record a second, possibly different, answer. What was learned before
                        // the refusal is kept.
                        break;
                    }
                }
            }
            if state != CredentialStore::Answered {
                break;
            }
        }
        let mut current = self.present.write().expect("the lock is not poisoned");
        current.extend(present);
        *self.store.write().expect("the lock is not poisoned") = state;
    }

    /// Whether one field is present in the store.
    #[must_use]
    pub fn field_present(&self, manifest: &IntegrationManifest, field: &CredentialField) -> bool {
        let key = manifest.secret_key(field);
        self.present
            .read()
            .expect("the lock is not poisoned")
            .contains(key.as_str())
    }

    /// Records that one field is now present or absent, after a write this process made.
    pub fn set_present(
        &self,
        manifest: &IntegrationManifest,
        field: &CredentialField,
        present: bool,
    ) {
        let key = manifest.secret_key(field);
        let mut current = self.present.write().expect("the lock is not poisoned");
        if present {
            current.insert(key.as_str().to_owned());
        } else {
            current.remove(key.as_str());
        }
    }

    /// The store failure as a display state, for a credential write that failed.
    pub fn note_store_failure(&self, error: &SecretStoreError) {
        *self.store.write().expect("the lock is not poisoned") = CredentialStore::of(error);
    }

    /// Parks a candidate for a job to pick up.
    pub fn park(&self, pending: PendingCredentials) -> u64 {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .expect("the lock is not poisoned")
            .insert(token, pending);
        token
    }

    /// Takes a parked candidate, exactly once.
    pub fn take(&self, token: u64) -> Option<PendingCredentials> {
        self.pending
            .lock()
            .expect("the lock is not poisoned")
            .remove(&token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peregrine_secrets::SecretStore;

    /// A store that answers with a fixed set of names.
    struct Fixed(Vec<String>, Option<SecretStoreError>);

    impl SecretStore for Fixed {
        fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
            if let Some(error) = self.1 {
                return Err(error);
            }
            Ok(self
                .0
                .iter()
                .find(|name| name.as_str() == key.as_str())
                .map(|_| Secret::new("fixture")))
        }

        fn set(&self, _key: &SecretKey, _secret: &Secret) -> Result<(), SecretStoreError> {
            Ok(())
        }

        fn delete(&self, _key: &SecretKey) -> Result<(), SecretStoreError> {
            Ok(())
        }
    }

    fn registry() -> IntegrationRegistry {
        IntegrationRegistry::new(Vec::new())
    }

    #[test]
    fn every_manifest_id_is_valid_and_unique() {
        let mut ids = HashSet::new();
        for entry in MANIFEST {
            assert!(ids.insert(entry.id), "{} is listed twice", entry.id);
            assert_eq!(entry.id().as_str(), entry.id);
        }
    }

    #[test]
    fn theintrodb_and_aniskip_are_configured_with_nothing_stored() {
        let status = registry().status();
        let intro = status
            .iter()
            .find(|row| row.manifest.id == "theintrodb")
            .unwrap();
        let ani = status
            .iter()
            .find(|row| row.manifest.id == "aniskip")
            .unwrap();

        assert!(intro.configured, "public reads need no key");
        assert!(ani.configured, "AniSkip has no credential at all");
        assert!(!intro.manifest.requires_key());
        assert!(intro.manifest.key_optional());
    }

    #[test]
    fn tmdb_is_not_configured_until_its_key_is_present() {
        let status = registry().status();
        let tmdb = status.iter().find(|row| row.manifest.id == "tmdb").unwrap();
        assert!(!tmdb.configured);

        let registry = registry();
        registry.refresh(&Fixed(vec!["provider.tmdb".to_owned()], None));
        let tmdb = registry
            .status()
            .into_iter()
            .find(|row| row.manifest.id == "tmdb")
            .unwrap();
        assert!(tmdb.configured);
    }

    #[test]
    fn chapters_is_enabled_by_default_and_network_sources_are_not() {
        let registry = registry();
        let enabled: Vec<&str> = registry.enabled().iter().map(|entry| entry.id).collect();

        assert_eq!(enabled, ["chapters"]);
    }

    #[test]
    fn enabled_sources_are_ordered_by_stored_priority() {
        let registry = registry();
        registry.remember(IntegrationPreference {
            id: IntegrationId::new("theintrodb").unwrap(),
            enabled: true,
            priority: 30,
            submission_enabled: false,
        });
        registry.remember(IntegrationPreference {
            id: IntegrationId::new("aniskip").unwrap(),
            enabled: true,
            priority: 5,
            submission_enabled: false,
        });

        let enabled: Vec<&str> = registry.enabled().iter().map(|entry| entry.id).collect();
        assert_eq!(enabled, ["chapters", "aniskip", "theintrodb"]);
    }

    #[test]
    fn a_denied_keyring_is_a_state_of_its_own() {
        let registry = registry();
        registry.refresh(&Fixed(Vec::new(), Some(SecretStoreError::Denied)));

        assert_eq!(registry.store_state(), CredentialStore::Denied);
        assert!(
            !registry
                .status()
                .into_iter()
                .find(|row| row.manifest.id == "tmdb")
                .unwrap()
                .configured,
            "a refusal cannot prove a key is absent, but it also cannot claim one is present"
        );
    }

    #[test]
    fn a_later_refusal_keeps_the_presence_a_previous_read_learned() {
        let registry = registry();
        registry.refresh(&Fixed(vec!["provider.tmdb".to_owned()], None));
        registry.refresh(&Fixed(Vec::new(), Some(SecretStoreError::Denied)));

        assert_eq!(registry.store_state(), CredentialStore::Denied);
        assert!(
            registry
                .status()
                .into_iter()
                .find(|row| row.manifest.id == "tmdb")
                .unwrap()
                .configured,
            "the adapter built from that key is still in hand"
        );
    }

    #[test]
    fn a_parked_candidate_is_taken_exactly_once() {
        let registry = registry();
        let id = IntegrationId::new("theintrodb").unwrap();
        let token = registry.park(PendingCredentials {
            integration: id,
            values: vec![("apiKey".to_owned(), Secret::new("fixture"))],
        });

        let taken = registry.take(token).expect("the first take finds it");
        assert_eq!(taken.values.len(), 1);
        assert!(registry.take(token).is_none());
    }
}
