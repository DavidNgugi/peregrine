//! The sync's two outcomes, against a fake provider and a real catalogue.

use std::num::NonZeroU32;

use peregrine_domain::catalogue::{
    ImageKind, MetadataState, TitleDetails, TitleKind, TitleSubtype,
};
use peregrine_domain::text::NonEmptyText;
use peregrine_domain::{AccountId, AccountScope, ExternalId, Provider};
use peregrine_providers::capability::{Capabilities, Capability};
use peregrine_providers::error::ProviderError;
use peregrine_providers::provider::{SearchCandidate, SearchQuery};
use peregrine_storage::{ArtworkPath, Database, StubTitle};
use peregrine_sync::{SyncOutcome, record_list_answer, refresh_title, sync_search};
use peregrine_testkit::{FakeMetadataProvider, fake_descriptor};

/// The id a fake candidate will answer with.
fn external(value: &str) -> ExternalId {
    ExternalId::new(Provider::Tmdb, value).expect("an id")
}

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
            ambiguous: 0,
            found: vec![external("603")],
        }
    );
    assert_eq!(
        second,
        SyncOutcome {
            matched: 1,
            created: 0,
            ambiguous: 0,
            found: vec![external("603")],
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
            ambiguous: 0,
            found: vec![external("603")],
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
    assert_eq!(written.details.poster_path.as_deref(), Some("/poster.jpg"));
    assert!(
        written.artwork.is_empty(),
        "a film has no season or episode pictures to fetch"
    );

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
        row.library.poster,
        Some(ArtworkPath {
            provider: Provider::Tmdb,
            path: "/poster.jpg".to_owned(),
        }),
        "the path a list answer carried is what the catalogue keeps"
    );
    assert!(
        !row.detailed,
        "a list answer is not a details read, however many fields it carried"
    );
    let identity = database
        .title_identity(scope(), id)
        .await
        .expect("a read")
        .expect("the title");
    assert_eq!(
        identity.metadata_state,
        peregrine_domain::catalogue::MetadataState::Listed,
        "and the state says so, which is what `refresh_due` reads"
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
        row.library.poster,
        Some(ArtworkPath {
            provider: Provider::Tmdb,
            path: "/poster.jpg".to_owned(),
        }),
        "a None must leave the stored path alone"
    );
    assert_eq!(
        row.library.title.metadata_state,
        MetadataState::Listed,
        "a list answer records the state that says a list answer is all that has happened, \
         which is what keeps a details read worth asking for"
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
async fn a_list_answer_carries_the_candidates_backdrop_to_the_writer() {
    // Discover's hero draws a wide image (`PG-E7-08`), and a list endpoint already sends one — so
    // this is the same link the score above is asserted through: the candidate's backdrop reaching
    // the write rather than being dropped on the way. **What it cannot pin** is the *stored* column:
    // `title.backdrop_path` is written by `update_title_metadata` and no read in `peregrine-storage`
    // selects it (`library_title` selects the poster's two columns), so the not-clearing half is the
    // `COALESCE` clause the artwork test above pins on the poster, which the two share.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = stub(&database, "The Matrix").await;

    let wide =
        listed("603", Some("/poster.jpg"), None).with_backdrop_path(Some("/wide.jpg".into()));
    let written = record_list_answer(&database, id, &wide, 2_000)
        .await
        .expect("the answer is written");

    assert_eq!(
        written.backdrop_path.as_deref(),
        Some("/wide.jpg"),
        "the wide path the hero needs is what the list answer was asked to write"
    );
    assert_eq!(written.poster_path.as_deref(), Some("/poster.jpg"));

    let bare = record_list_answer(&database, id, &listed("603", None, None), 3_000)
        .await
        .expect("the second answer is written");
    assert_eq!(
        bare.backdrop_path, None,
        "an answer with no wide image carries none — it does not reuse the poster"
    );
}

#[tokio::test]
async fn a_list_answer_carries_the_synopsis_and_the_vote_count_it_was_given() {
    // The hover preview draws both from the rail item the list answer produced (`PG-E7-05`), and
    // neither is a column any repository query selects — so, exactly as with the score above, what
    // this pins is the sync's own link: the candidate's synopsis and count reach the write rather
    // than being dropped on the way. The not-clearing half is `COALESCE`, the same clause the date
    // and the artwork path are asserted against.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let id = stub(&database, "The Matrix").await;

    let described = listed("603", None, Some(8.2))
        .with_overview(Some("A hacker learns the truth.".to_owned()))
        .with_votes(Some(24_000));
    let written = record_list_answer(&database, id, &described, 2_000)
        .await
        .expect("the answer is written");

    assert_eq!(
        written.overview.as_deref(),
        Some("A hacker learns the truth.")
    );
    assert_eq!(written.votes, Some(24_000));

    let bare = record_list_answer(&database, id, &listed("603", None, Some(8.2)), 3_000)
        .await
        .expect("the second answer is written");
    assert_eq!(bare.overview, None, "the second answer carried no synopsis");
    assert_eq!(bare.votes, None, "the second answer carried no count");
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

/// A series the catalogue holds, with a TMDB id attached, and the provider that describes it.
async fn described_show(
    database: &Database,
    details: TitleDetails,
    seasons: Vec<peregrine_domain::catalogue::SeasonDetails>,
) -> (i64, FakeMetadataProvider) {
    let id = database
        .insert_stub_title(StubTitle {
            kind: TitleKind::Show,
            subtype: TitleSubtype::Standard,
            name: details.name.as_str().to_owned(),
            original_name: None,
            sort_name: details.name.as_str().to_lowercase(),
            at: 1_000,
        })
        .await
        .expect("a stub");
    database
        .attach_external_ids(
            id,
            &[peregrine_storage::TitleExternalId {
                id: ExternalId::new(Provider::Tmdb, "1396").expect("an id"),
                is_primary: true,
                fetched_at: 1_000,
            }],
        )
        .await
        .expect("the id attaches");
    (
        id,
        FakeMetadataProvider::describing_episodes(Provider::Tmdb, details, seasons),
    )
}

fn show_details() -> TitleDetails {
    TitleDetails {
        kind: TitleKind::Show,
        name: NonEmptyText::new("Breaking Bad").expect("a name"),
        original_name: None,
        overview: None,
        tagline: None,
        released: Some("2008-01-20".parse().expect("a date")),
        runtime_minutes: None,
        original_language: None,
        origin_country: None,
        rating: None,
        votes: None,
        popularity: None,
        adult: None,
        in_production: None,
        season_count: Some(2),
        episode_count: Some(9),
        homepage: None,
        poster_path: Some("/poster.jpg".to_owned()),
        backdrop_path: None,
    }
}

fn season_details(number: u32, poster: Option<&str>) -> peregrine_domain::catalogue::SeasonDetails {
    peregrine_domain::catalogue::SeasonDetails {
        season_number: peregrine_domain::catalogue::SeasonNumber::new(number),
        name: Some(format!("Season {number}")),
        overview: None,
        air_date: Some("2008-01-20".parse().expect("a date")),
        poster_path: poster.map(str::to_owned),
        episode_count: Some(2),
    }
}

fn episode_details(
    season: u32,
    episode: u32,
    still: Option<&str>,
) -> peregrine_domain::catalogue::EpisodeDetails {
    peregrine_domain::catalogue::EpisodeDetails {
        season_number: peregrine_domain::catalogue::SeasonNumber::new(season),
        episode_number: peregrine_domain::catalogue::EpisodeNumber::new(episode),
        name: Some(format!("Episode {episode}")),
        overview: Some("An episode.".to_owned()),
        air_date: still.map(|_| "2008-01-27".parse().expect("a date")),
        runtime_minutes: Some(47),
        still_path: still.map(str::to_owned),
        rating: Some(8.1),
        votes: Some(1_200),
        episode_type: None,
    }
}

#[tokio::test]
async fn a_series_refresh_writes_its_seasons_and_their_episodes_and_hands_back_the_pictures() {
    // The write side of the title page: two reads — the season list and one per season — and every
    // field a switcher and an episode card draw. The pictures come back as provider-relative paths
    // because the blobs belong in the image cache and this layer has none (`AGENTS R11`).
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let (id, provider) = described_show(
        &database,
        show_details(),
        vec![season_details(1, Some("/s1.jpg")), season_details(2, None)],
    )
    .await;
    let provider = provider
        .with_episodes(1, vec![episode_details(1, 1, Some("/s1e1.jpg"))])
        .with_episodes(2, vec![episode_details(2, 1, None)]);

    let written = refresh_title(&provider, &database, id, TitleKind::Show, 2_000)
        .await
        .expect("the refresh runs")
        .expect("the provider serves details");

    assert_eq!(
        written.artwork,
        vec![
            (ImageKind::Poster, "/s1.jpg".to_owned()),
            (ImageKind::Still, "/s1e1.jpg".to_owned()),
        ],
        "a season with no poster and an episode with no still contribute nothing to fetch"
    );
    let scope = peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("an account"),
    );
    let seasons = database
        .list_title_seasons(scope, id, 10)
        .await
        .expect("the season read");
    assert_eq!(seasons.len(), 2);
    assert_eq!(seasons[0].name.as_deref(), Some("Season 1"));
    assert_eq!(
        seasons[0]
            .poster
            .as_ref()
            .map(|artwork| artwork.path.as_str()),
        Some("/s1.jpg")
    );
    let episodes = database
        .list_title_episodes(scope, id, Some(1), 10)
        .await
        .expect("the episode read");
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].name.as_deref(), Some("Episode 1"));
    assert_eq!(
        episodes[0]
            .still
            .as_ref()
            .map(|artwork| artwork.path.as_str()),
        Some("/s1e1.jpg")
    );
    assert_eq!(
        episodes[0].air_date,
        Some("2008-01-27".parse().expect("a date"))
    );
}

#[tokio::test]
async fn a_series_refresh_records_that_the_season_step_was_answered() {
    // The fact the title page's refresh rule reads. Without it, "details read" and "everything a
    // provider can give" were the same answer, and a show whose season step had not happened looked
    // finished — which is a page with no seasons and no reason to ask again.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let (id, provider) =
        described_show(&database, show_details(), vec![season_details(1, None)]).await;
    let provider = provider.with_episodes(1, vec![episode_details(1, 1, None)]);

    refresh_title(&provider, &database, id, TitleKind::Show, 2_000)
        .await
        .expect("the refresh runs")
        .expect("the provider serves details");

    let scope = peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("an account"),
    );
    let row = database
        .library_title(scope, id)
        .await
        .expect("the title read")
        .expect("the fixture's title");
    assert!(
        row.seasons_synced,
        "the season step answered, so the title is no longer due for it"
    );
}

#[tokio::test]
async fn a_season_list_that_failed_leaves_the_title_due_to_be_asked_again() {
    // The other half, and the one that must not regress: the details are written first, so a marker
    // written before the season read would make a provider's bad minute permanent. The column stays
    // empty and the next visit asks again.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let (id, provider) = described_show(&database, show_details(), Vec::new()).await;
    let provider = provider.with_season_list_failure(ProviderError::RateLimited {
        provider: Provider::Tmdb,
        retry_after: None,
    });

    refresh_title(&provider, &database, id, TitleKind::Show, 2_000)
        .await
        .expect_err("a rate limit is the provider's problem and fails the refresh");

    let scope = peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("an account"),
    );
    let row = database
        .library_title(scope, id)
        .await
        .expect("the title read")
        .expect("the fixture's title");
    assert!(
        row.detailed,
        "the details were written before the season read, which is why this needed its own fact"
    );
    assert!(
        !row.seasons_synced,
        "a failed season read must leave the title due"
    );
}

#[tokio::test]
async fn a_season_the_provider_does_not_hold_does_not_stop_its_siblings_being_written() {
    // One season's 404 is that season's problem: a series whose answer is missing one season still has
    // the others, and failing the whole refresh would discard what did answer. It is also the shape a
    // title page hits in practice — a specials season TMDB has no episodes for is the common case.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let (id, provider) = described_show(
        &database,
        show_details(),
        vec![
            season_details(1, None),
            season_details(2, None),
            season_details(3, Some("/s3.jpg")),
        ],
    )
    .await;
    let provider = provider
        .with_episodes(1, vec![episode_details(1, 1, None)])
        .with_season_failure(
            2,
            ProviderError::NotFound {
                provider: Provider::Tmdb,
            },
        )
        .with_episodes(3, vec![episode_details(3, 1, None)]);

    let written = refresh_title(&provider, &database, id, TitleKind::Show, 2_000)
        .await
        .expect("a missing season is not a failed refresh")
        .expect("the provider serves details");

    assert_eq!(
        written.artwork,
        vec![(ImageKind::Poster, "/s3.jpg".to_owned())],
        "the seasons that answered still contributed their pictures"
    );
    let scope = peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("an account"),
    );
    let seasons = database
        .list_title_seasons(scope, id, 10)
        .await
        .expect("the season read");
    assert_eq!(
        seasons.len(),
        3,
        "the season that 404'd is still a season the list named"
    );
    let episodes = database
        .list_title_episodes(scope, id, None, 10)
        .await
        .expect("the episode read");
    let numbers: Vec<u32> = episodes
        .iter()
        .map(|episode| episode.season_number.get())
        .collect();
    assert_eq!(
        numbers,
        vec![1, 3],
        "the seasons either side of the missing one were written"
    );
}

#[tokio::test]
async fn a_season_with_no_episodes_is_written_with_an_empty_episode_list() {
    // The other shape of "nothing": the season list published season zero, and the season read answered
    // an empty list. A season row is what the switcher draws, so it exists — and the episode read is
    // empty rather than absent.
    let directory = tempfile::tempdir().expect("a directory");
    let database = database(&directory).await;
    let (id, provider) =
        described_show(&database, show_details(), vec![season_details(0, None)]).await;
    let provider = provider.with_episodes(0, Vec::new());

    refresh_title(&provider, &database, id, TitleKind::Show, 2_000)
        .await
        .expect("the refresh runs")
        .expect("the provider serves details");

    let scope = peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("an account"),
    );
    let seasons = database
        .list_title_seasons(scope, id, 10)
        .await
        .expect("the season read");
    assert_eq!(seasons.len(), 1);
    assert_eq!(seasons[0].season_number.get(), 0);
    assert_eq!(
        database
            .list_title_episodes(scope, id, Some(0), 10)
            .await
            .expect("the episode read")
            .len(),
        0
    );
}
