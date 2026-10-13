//! The sync's two outcomes, against a fake provider and a real catalogue.

use std::num::NonZeroU32;

use peregrine_domain::catalogue::{MetadataState, TitleDetails, TitleKind, TitleSubtype};
use peregrine_domain::text::NonEmptyText;
use peregrine_domain::{AccountId, AccountScope, ExternalId, Provider};
use peregrine_providers::capability::{Capabilities, Capability};
use peregrine_providers::provider::{SearchCandidate, SearchQuery};
use peregrine_storage::{ArtworkPath, Database, StubTitle};
use peregrine_sync::{SyncOutcome, record_list_answer, refresh_title, sync_search};
use peregrine_testkit::{FakeMetadataProvider, fake_descriptor};

fn candidate(value: &str, name: &str, released: Option<&str>) -> SearchCandidate {
    SearchCandidate::new(
        ExternalId::new(Provider::Tmdb, value).expect("an id"),
        NonEmptyText::new(name).expect("a name"),
        TitleKind::Movie,
        released.map(|date| date.parse().expect("a date")),
    )
}

/// One candidate as a *list* endpoint answers it, with the two fields a list adds.
fn listed(value: &str, artwork: Option<&str>, rating: Option<f64>) -> SearchCandidate {
    candidate(value, "The Matrix", Some("1999-03-31"))
        .with_artwork_path(artwork.map(str::to_owned))
        .with_rating(rating)
}

/// A stub for a film, at the id the caller can then fill.
async fn stub(database: &Database, name: &str) -> i64 {
    database
        .insert_stub_title(StubTitle {
            kind: TitleKind::Movie,
            subtype: TitleSubtype::Standard,
            name: name.to_owned(),
            original_name: None,
            sort_name: name.to_lowercase(),
            at: 1_000,
        })
        .await
        .expect("a stub")
}

fn scope() -> AccountScope {
    AccountScope::unrestricted(AccountId::new(1).expect("account one"))
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

#[tokio::test]
async fn a_list_answer_fills_an_empty_title_with_the_artwork_it_carried() {
    // The point of the list answer: the poster path a rail page already sent is kept, so the screen
    // does not have to ask the provider for the same title a second time (`PG-E7-08`).
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = stub(&database, "The Matrix").await;

    record_list_answer(
        &database,
        id,
        &listed("603", Some("/poster.jpg"), None),
        2_000,
    )
    .await
    .expect("the list answer is written");

    let row = database
        .library_title(scope(), id)
        .await
        .expect("a read")
        .expect("the title");
    assert_eq!(
        row.poster,
        Some(ArtworkPath {
            provider: Provider::Tmdb,
            path: "/poster.jpg".to_owned(),
        }),
        "the path a list answer carried is what the catalogue keeps"
    );
    assert!(
        row.synced,
        "a provider answered about the title, which is what last_synced_at records"
    );
}

#[tokio::test]
async fn a_later_list_answer_that_carries_no_artwork_does_not_clear_the_stored_one() {
    // PG-E5-07's rule, at the list-answer boundary: a provider omits freely, so an omission is not
    // an instruction to erase. A second run over the same rail sends the same paths today, and a
    // provider that stopped sending them tomorrow must not blank a poster the cache still holds.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = stub(&database, "The Matrix").await;

    record_list_answer(
        &database,
        id,
        &listed("603", Some("/poster.jpg"), None),
        2_000,
    )
    .await
    .expect("the first answer is written");
    record_list_answer(&database, id, &listed("603", None, None), 3_000)
        .await
        .expect("the second answer is written");

    let row = database
        .library_title(scope(), id)
        .await
        .expect("a read")
        .expect("the title");
    assert_eq!(
        row.poster,
        Some(ArtworkPath {
            provider: Provider::Tmdb,
            path: "/poster.jpg".to_owned(),
        }),
        "a None must leave the stored path alone"
    );
    assert_eq!(
        row.library.title.metadata_state,
        MetadataState::Partial,
        "a list answer is a partial fill, the state a details read records too"
    );
}

#[tokio::test]
async fn a_list_answer_carries_the_candidates_score_to_the_writer() {
    // **What this can and cannot pin, stated rather than implied.** `title.tmdb_rating` is written
    // by `update_title_metadata`'s `COALESCE(?12, tmdb_rating)` and is read by no query in
    // `peregrine-storage`, so there is no public read to assert the *stored* score against: the
    // not-clearing half is the same clause as the artwork path, and
    // `a_later_list_answer_that_carries_no_artwork_does_not_clear_the_stored_one` pins it on the
    // column that can be read back. What this test owns is the link the sync is responsible for —
    // the candidate's score reaching the write rather than being dropped on the way.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = stub(&database, "The Matrix").await;

    let written = record_list_answer(&database, id, &listed("603", None, Some(8.2)), 2_000)
        .await
        .expect("the answer is written");
    assert_eq!(written.rating, Some(8.2));
    assert_eq!(written.poster_path, None, "this answer carried no artwork");

    let second = record_list_answer(&database, id, &listed("603", None, None), 3_000)
        .await
        .expect("the second answer is written");
    assert_eq!(second.rating, None, "the second answer carried no score");
}

#[tokio::test]
async fn a_list_answer_never_clears_a_field_a_details_fill_stored() {
    // The rule PG-E5-07 established, across the two writers rather than within one: a date a
    // *details* read recorded survives a later list answer that omits it, because every optional
    // column goes through `COALESCE` — a bare `= ?` here would erase it, and the score the same
    // statement carries rides the same clause. Asserted on the date, which is a column the
    // repository can read back (`tmdb_rating` is written and never selected by any query).
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = stub(&database, "The Matrix").await;
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
    let describing = FakeMetadataProvider::describing(
        Provider::Tmdb,
        TitleDetails {
            kind: TitleKind::Movie,
            name: NonEmptyText::new("The Matrix").expect("a name"),
            original_name: None,
            overview: None,
            tagline: None,
            released: Some("1999-03-31".parse().expect("a date")),
            runtime_minutes: None,
            original_language: None,
            origin_country: None,
            rating: Some(8.2),
            votes: None,
            popularity: None,
            adult: None,
            in_production: None,
            season_count: None,
            episode_count: None,
            homepage: None,
            poster_path: None,
            backdrop_path: None,
        },
    );
    refresh_title(&describing, &database, id, TitleKind::Movie, 2_000)
        .await
        .expect("the details fill runs");

    // A list answer that carries no date at all — an item with no `release_date`, which is a real
    // shape rather than a fixture's convenience.
    let undated = SearchCandidate::new(
        ExternalId::new(Provider::Tmdb, "603").expect("an id"),
        NonEmptyText::new("The Matrix").expect("a name"),
        TitleKind::Movie,
        None,
    );
    record_list_answer(&database, id, &undated, 3_000)
        .await
        .expect("the list answer is written");

    let locals = database
        .local_titles_for_sort_name("movie", "the matrix")
        .await
        .expect("the matching read");
    assert_eq!(locals.len(), 1);
    assert_eq!(
        locals[0].released,
        Some("1999-03-31".parse().expect("a date")),
        "a date a details fill stored must survive a list answer that omits it"
    );
}
