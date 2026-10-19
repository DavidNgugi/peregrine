//! A scripted Usenet engine, and the conformance suite its port is judged by.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use peregrine_domain::{FileSelection, InspectionLimits, OptIn, PathGuard, PayloadInspector};
use peregrine_usenet::{
    AddNzbRequest, EngineError, EngineFuture, NzbDocument, UsenetEngine, UsenetHandle, UsenetStats,
};

/// A fake that holds one deterministic job per add and records nothing about wiring.
#[derive(Default)]
pub struct FakeUsenetEngine {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next: u64,
    jobs: BTreeMap<u64, UsenetStats>,
    closed: bool,
}

impl FakeUsenetEngine {
    /// A fresh session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl UsenetEngine for FakeUsenetEngine {
    fn descriptor(&self) -> (String, String) {
        ("fake".to_owned(), "1".to_owned())
    }

    fn add(&self, request: AddNzbRequest) -> EngineFuture<'_, UsenetHandle> {
        Box::pin(async move {
            let mut state = self.lock();
            if state.closed {
                return Err(EngineError::SessionClosed);
            }
            state.next += 1;
            let handle = UsenetHandle::new(state.next);
            let selected = request.selection();
            let total_bytes = request
                .document()
                .files()
                .iter()
                .enumerate()
                .filter(|(index, _)| selected.contains(*index))
                .fold(0u64, |total, (_, file)| total.saturating_add(file.bytes()));
            let articles_total = request
                .document()
                .files()
                .iter()
                .enumerate()
                .filter(|(index, _)| selected.contains(*index))
                .map(|(_, file)| file.segments.len())
                .sum::<usize>();
            state.jobs.insert(
                handle.get(),
                UsenetStats {
                    downloaded_bytes: 0,
                    total_bytes,
                    articles_done: 0,
                    articles_total: u32::try_from(articles_total).unwrap_or(u32::MAX),
                    articles_missing: 0,
                    rate_bps: 0,
                    connections: 1,
                    eta_seconds: None,
                    complete: false,
                    error: None,
                },
            );
            Ok(handle)
        })
    }

    fn stats(&self, handle: UsenetHandle) -> EngineFuture<'_, UsenetStats> {
        Box::pin(async move {
            let state = self.lock();
            if state.closed {
                return Err(EngineError::SessionClosed);
            }
            state
                .jobs
                .get(&handle.get())
                .cloned()
                .ok_or(EngineError::UnknownHandle)
        })
    }

    fn pause(&self, handle: UsenetHandle) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            let state = self.lock();
            if state.closed {
                return Err(EngineError::SessionClosed);
            }
            state
                .jobs
                .contains_key(&handle.get())
                .then_some(())
                .ok_or(EngineError::UnknownHandle)
        })
    }

    fn resume(&self, handle: UsenetHandle) -> EngineFuture<'_, ()> {
        self.pause(handle)
    }

    fn remove(&self, handle: UsenetHandle) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock();
            if state.closed {
                return Err(EngineError::SessionClosed);
            }
            state.jobs.remove(&handle.get());
            Ok(())
        })
    }

    fn shutdown(&self) -> EngineFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock();
            state.closed = true;
            state.jobs.clear();
            Ok(())
        })
    }
}

/// Asserts the port's contract against one engine.
///
/// # Panics
///
/// When any operation refuses or answers the wrong thing.
pub async fn usenet_engine_satisfies_the_port(engine: &dyn UsenetEngine) {
    let (name, version) = engine.descriptor();
    assert!(!name.is_empty(), "an engine names itself");
    assert!(!version.is_empty(), "an engine names its version");

    let root = std::env::temp_dir();
    let guard = PathGuard::new(&root).expect("the temporary directory is an absolute root");
    let nzb = "<nzb><file subject=\"file.mkv\" date=\"0\" poster=\"p\">\
               <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
               </segments></file></nzb>";
    let document = NzbDocument::parse(nzb.as_bytes()).expect("the fixture parses");
    let report = PayloadInspector::new(InspectionLimits::new(100, u64::MAX)).inspect(
        &document.description(),
        &guard,
        OptIn::NotGranted,
    );
    let save_path = guard
        .resolve(&root)
        .expect("the temporary directory resolves");
    let request = AddNzbRequest::new(
        document,
        save_path,
        FileSelection::new([0]),
        Vec::new(),
        &report,
    )
    .expect("an allowed report spells an add");

    let handle = engine.add(request).await.expect("the add starts");
    let stats = engine.stats(handle).await.expect("the job answers");
    assert!(stats.total_bytes > 0, "the fake counts the post");
    engine.pause(handle).await.expect("pause");
    engine.resume(handle).await.expect("resume");
    engine.remove(handle).await.expect("remove");
    assert_eq!(
        engine.stats(handle).await,
        Err(EngineError::UnknownHandle),
        "a removed handle is unknown"
    );
    engine.shutdown().await.expect("shutdown");
    let refused = engine
        .add(
            AddNzbRequest::new(
                NzbDocument::parse(nzb.as_bytes()).expect("the fixture parses"),
                guard.resolve(&root).expect("resolves"),
                FileSelection::new([0]),
                Vec::new(),
                &report,
            )
            .expect("an allowed report"),
        )
        .await;
    assert_eq!(refused, Err(EngineError::SessionClosed));
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[test]
    fn the_fake_satisfies_the_port() {
        block_on(usenet_engine_satisfies_the_port(&FakeUsenetEngine::new()));
    }
}
