//! Test doubles shared by this crate's unit tests.
//!
//! [`RecordingSink`] is the one that matters: the event bus's only collaborator is its
//! sink, and the tests that prove the delivery rate, the coalescing, and the degraded
//! signal all need the same sink that records what it received.
//!
//! [`CountingSecretStore`] exists for the rules that are about a keyring call *not*
//! happening: a refused credential never reaches the store (PG-E5-20), and a polled status
//! answers from the registry's cache without touching one. Counting is how both are
//! asserted without a real keychain anywhere near a test (AGENTS R14).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};

use crate::events::{Event, EventSink, EventSinkError};

/// A sink that records every event, and can be told to refuse them.
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<Event>>,
    refuse: bool,
}

impl RecordingSink {
    /// A sink whose transport always fails, for the degraded-signal tests.
    #[must_use]
    pub fn refusing() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            refuse: true,
        }
    }

    /// Everything delivered so far, in delivery order.
    #[must_use]
    pub fn delivered(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &Event) -> Result<(), EventSinkError> {
        if self.refuse {
            return Err(EventSinkError::new("the transport is gone"));
        }
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

/// A secret store that keeps secrets in memory, counts every call, and can be told to be
/// unavailable.
#[derive(Debug, Default)]
pub struct CountingSecretStore {
    values: Mutex<HashMap<String, String>>,
    reads: AtomicUsize,
    writes: AtomicUsize,
    deletes: AtomicUsize,
    /// The thread the most recent call was made on, which is how a test proves a keyring call
    /// ran on a blocking thread rather than inline on the runtime (PG-E5-12).
    caller: Mutex<Option<std::thread::ThreadId>>,
    unavailable: bool,
}

impl CountingSecretStore {
    /// An empty store every call succeeds against.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose every call answers [`SecretStoreError::Unavailable`], for the
    /// keyring-is-missing path.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    /// How many reads the store answered.
    #[must_use]
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    /// How many writes were attempted, whether or not they succeeded.
    #[must_use]
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    /// How many deletes were attempted.
    #[must_use]
    pub fn deletes(&self) -> usize {
        self.deletes.load(Ordering::SeqCst)
    }

    /// The thread the most recent call was made on, or `None` when no call was made.
    #[must_use]
    pub fn caller(&self) -> Option<std::thread::ThreadId> {
        *self.caller.lock().expect("the lock is not poisoned")
    }

    /// Whether a value is stored for `provider`, without counting as a read.
    #[must_use]
    pub fn holds(&self, provider: &str) -> bool {
        self.values
            .lock()
            .expect("the lock is not poisoned")
            .contains_key(SecretKey::provider(provider).as_str())
    }

    /// What every call answers when the store is available.
    fn answer(&self) -> Result<(), SecretStoreError> {
        if self.unavailable {
            return Err(SecretStoreError::Unavailable);
        }
        Ok(())
    }

    /// Notes which thread is answering, before the answer is decided.
    fn entered(&self) {
        *self.caller.lock().expect("the lock is not poisoned") = Some(std::thread::current().id());
    }
}

impl SecretStore for CountingSecretStore {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.entered();
        self.answer()?;
        let values = self.values.lock().expect("the lock is not poisoned");
        Ok(values.get(key.as_str()).map(Secret::new))
    }

    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.entered();
        self.answer()?;
        let mut values = self.values.lock().expect("the lock is not poisoned");
        values.insert(key.as_str().to_owned(), secret.expose_secret().to_owned());
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        self.entered();
        self.answer()?;
        let mut values = self.values.lock().expect("the lock is not poisoned");
        values.remove(key.as_str());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CountingSecretStore;
    use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};

    #[test]
    fn the_counting_store_counts_what_it_answered() {
        let store = CountingSecretStore::new();
        let key = SecretKey::provider("tmdb");

        assert!(store.get(&key).expect("a read").is_none());
        store.set(&key, &Secret::new("a-key")).expect("a write");
        assert!(store.holds("tmdb"));
        store.delete(&key).expect("a delete");

        assert_eq!((store.reads(), store.writes(), store.deletes()), (1, 1, 1));
        assert!(!store.holds("tmdb"));
    }

    #[test]
    fn an_unavailable_store_refuses_every_call_and_still_counts_it() {
        let store = CountingSecretStore::unavailable();
        let key = SecretKey::provider("tmdb");

        assert_eq!(store.get(&key).err(), Some(SecretStoreError::Unavailable));
        assert_eq!(
            store.set(&key, &Secret::new("a-key")),
            Err(SecretStoreError::Unavailable)
        );
        assert_eq!(store.delete(&key), Err(SecretStoreError::Unavailable));
        assert_eq!((store.reads(), store.writes(), store.deletes()), (1, 1, 1));
    }
}
