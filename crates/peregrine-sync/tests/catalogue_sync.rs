//! The sync's two outcomes, against a fake provider and a real catalogue.

use std::num::NonZeroU32;

use peregrine_domain::catalogue::{TitleKind, TitleSubtype};
use peregrine_domain::text::NonEmptyText;
use peregrine_domain::{ExternalId, Provider};
use peregrine_providers::capability::{Capabilities, Capability};
use peregrine_providers::provider::{SearchCandidate, SearchQuery};
use peregrine_storage::{Database, StubTitle};
use peregrine_sync::{SyncOutcome, sync_search};
use peregrine_testkit::{FakeMetadataProvider, fake_descriptor};

fn candidate(value: &str, name: &str, released: Option<&str>) -> SearchCandidate {
    SearchCandidate::new(
        ExternalId::new(Provider::Tmdb, value).expect("an id"),
        NonEmptyText::new(name).expect("a name"),
        TitleKind::Movie,
        released.map(|date| date.parse().expect("a date")),
    )
}

fn query() -> SearchQuery {
    SearchQuery::new(
        NonEmptyText::new("matrix").expect("a query"),
        NonZeroU32::new(20).expect("a limit"),
        None,
    )
}

/// A provider that answers with `candidates` to anything.
fn provider(candidates: Vec<SearchCandidate>) -> FakeMetadataProvider {
    FakeMetadataProvider::new(
        fake_descriptor(Provider::Tmdb, Capabilities::EMPTY.with(Capability::Search)),
        Ok(candidates),
    )
}

async fn database(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("peregrine.sqlite"))
        .await
        .expect("a database")
}

#[tokio::test]
async fn a_candidate_nobody_has_creates_one_title_and_the_second_sync_matches_it() {
    // The acceptance's real question: syncing twice must not make two titles. The first run creates the
    // stub and attaches the id; the second finds the id and writes nothing.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let provider = provider(vec![candidate("603", "The Matrix", Some("1999-03-31"))]);

    let first = sync_search(&provider, &database, &query(), 1_000)
        .await
        .expect("the sync runs");
    let second = sync_search(&provider, &database, &query(), 2_000)
        .await
        .expect("the sync runs again");

    assert_eq!(
        first,
        SyncOutcome {
            matched: 0,
            created: 1,
            ambiguous: 0
        }
    );
    assert_eq!(
        second,
        SyncOutcome {
            matched: 1,
            created: 0,
            ambiguous: 0
        }
    );
    assert_eq!(database.title_count().await.expect("a count"), 1);
}

#[tokio::test]
async fn a_candidate_that_names_a_title_already_held_attaches_the_id_to_it() {
    // The library indexed this film from a file and never knew its tmdb id. The sync's job is to give it
    // one, not to make a second row for it.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let held = database
        .insert_stub_title(StubTitle {
            kind: TitleKind::Movie,
            subtype: TitleSubtype::Standard,
            name: "The Matrix".to_owned(),
            original_name: None,
            sort_name: "the matrix".to_owned(),
            at: 1_000,
        })
        .await
        .expect("a stub");

    let outcome = sync_search(
        &provider(vec![candidate("603", "The Matrix", None)]),
        &database,
        &query(),
        2_000,
    )
    .await
    .expect("the sync runs");

    assert_eq!(
        outcome,
        SyncOutcome {
            matched: 1,
            created: 0,
            ambiguous: 0
        }
    );
    assert_eq!(database.title_count().await.expect("a count"), 1);
    assert_eq!(
        database
            .title_for_external_id(&ExternalId::new(Provider::Tmdb, "603").expect("an id"))
            .await
            .expect("a lookup"),
        Some(held)
    );
}
