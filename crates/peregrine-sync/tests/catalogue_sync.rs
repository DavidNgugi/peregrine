//! The sync's two outcomes, against a fake provider and a real catalogue.

use std::num::NonZeroU32;

use peregrine_domain::catalogue::{TitleDetails, TitleKind, TitleSubtype};
use peregrine_domain::text::NonEmptyText;
use peregrine_domain::{ExternalId, Provider};
use peregrine_providers::capability::{Capabilities, Capability};
use peregrine_providers::provider::{SearchCandidate, SearchQuery};
use peregrine_storage::{Database, StubTitle};
use peregrine_sync::{SyncOutcome, refresh_title, sync_search};
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

#[tokio::test]
async fn a_stub_is_filled_from_the_provider_that_knows_it() {
    // The point of a stub: the library created the row, and the provider is what makes it a title with a
    // synopsis, a rating, and a date.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = database
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
    database
        .attach_external_ids(
            id,
            &[peregrine_storage::TitleExternalId {
                id: ExternalId::new(Provider::Tmdb, "603").expect("an id"),
                is_primary: true,
                fetched_at: 1_000,
            }],
        )
        .await
        .expect("the id attaches");

    let provider = FakeMetadataProvider::describing(
        Provider::Tmdb,
        TitleDetails {
            kind: TitleKind::Movie,
            name: NonEmptyText::new("The Matrix").expect("a name"),
            original_name: None,
            overview: Some("A hacker learns the truth.".to_owned()),
            tagline: None,
            released: Some("1999-03-31".parse().expect("a date")),
            runtime_minutes: Some(136),
            original_language: Some("en".to_owned()),
            origin_country: None,
            rating: Some(8.2),
            votes: Some(24_000),
            popularity: Some(42.0),
            adult: Some(false),
            in_production: None,
            season_count: None,
            episode_count: None,
            homepage: None,
            poster_path: Some("/poster.jpg".to_owned()),
            backdrop_path: None,
        },
    );

    let written = refresh_title(&provider, &database, id, TitleKind::Movie, 2_000)
        .await
        .expect("the refresh runs")
        .expect("the provider serves details");
    // The details come back because the caller needs the artwork paths they carry.
    assert_eq!(written.poster_path.as_deref(), Some("/poster.jpg"));

    let scope = peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("an account"),
    );
    let identity = database
        .title_identity(scope, id)
        .await
        .expect("a read")
        .expect("the title");
    assert_eq!(
        identity.metadata_state,
        peregrine_domain::catalogue::MetadataState::Partial
    );
}
