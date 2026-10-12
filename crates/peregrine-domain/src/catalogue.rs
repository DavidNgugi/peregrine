//! The catalogue: titles, the names they go by, who made them, and how their episodes are
//! numbered.
//!
//! One type per table in `DATA_MODEL.md` §2.2 and §2.3, with a field per column and a field
//! type that carries the column's constraint. That is why most of these structs have public
//! fields and no fallible `new`: there is nothing left for a constructor to check once an
//! enum replaces every `CHECK`, [`crate::text::NonEmptyText`] replaces every required text
//! column, `bool` replaces every `0/1` column, and `Option` marks every nullable one. A
//! relationship *between* columns is the one thing a field type cannot carry, and that is the
//! only case that gets a constructor (see [`crate::library::WatchProgress`]).
//!
//! Row and foreign-key ids are plain `i64` because they are the database's to assign: nothing
//! in the domain computes them, and there is no rule about them that a read or a write could
//! break by mixing two of them up beyond what the schema's foreign keys already refuse.
//!
//! The enums are the source of truth for the `CHECK` constraints they mirror, exactly as
//! `DATA_MODEL` §2 requires, and each one lists its values in the constraint's order. A test
//! here pins those spellings and that order as a regression guard, which is the strongest
//! check a pure crate can make: it may not read a database (AGENTS R6). The check that the
//! enum and the schema actually agree lives in `peregrine-storage`, which reads the `CHECK`
//! clause out of `sqlite_master` and compares it with these same enums in both directions.

use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::stored_enum;
use crate::text::NonEmptyText;

stored_enum! {
    /// A title's kind, as `title.kind` stores it.
    TitleKind {
        /// A film.
        Movie => "movie",
        /// A series.
        Show => "show",
    }
}

stored_enum! {
    /// A title's subtype, as `title.subtype` stores it.
    ///
    /// A `CHECK` rather than a boolean because anime and documentary affect provider
    /// selection and episode ordering, and widening a constraint is a cheap migration.
    TitleSubtype {
        /// An ordinary film or series.
        Standard => "standard",
        /// Animated, ordered by absolute numbering as often as not.
        Anime => "anime",
        /// Non-fiction.
        Documentary => "documentary",
        /// Unscripted.
        Reality => "reality",
    }
}

stored_enum! {
    /// A title's production status, as `title.status` stores it.
    TitleStatus {
        /// Announced but not green-lit.
        Rumored => "rumored",
        /// Green-lit, not yet filming.
        Planned => "planned",
        /// Filming or in post.
        InProduction => "in_production",
        /// Airing, with more episodes to come.
        Returning => "returning",
        /// Finished on its own terms.
        Ended => "ended",
        /// Stopped early.
        Canceled => "canceled",
        /// Released.
        Released => "released",
        /// Dated but not released.
        Upcoming => "upcoming",
    }
}

stored_enum! {
    /// How complete a title's metadata is, as `title.metadata_state` stores it.
    ///
    /// `Stale` and `Failed` are not errors: what is stored is still shown, and the state says
    /// whether a refresh is due or the last one failed.
    MetadataState {
        /// The identity exists and nothing else does.
        Stub => "stub",
        /// Some fields are filled.
        Partial => "partial",
        /// Every field the providers supply is filled.
        Complete => "complete",
        /// Filled, but past its refresh window.
        Stale => "stale",
        /// The last refresh failed.
        Failed => "failed",
    }
}

stored_enum! {
    /// Where an alternate name came from, as `title_alias.kind` stores it.
    AliasKind {
        /// A name used in another market or language.
        Alternative => "alternative",
        /// A name used before release.
        Working => "working",
        /// The name the title was made under.
        Original => "original",
    }
}

stored_enum! {
    /// What a company did on a title, as `title_company.role` stores it.
    CompanyRole {
        /// The channel or service that aired it.
        Network => "network",
        /// The studio that made it.
        Production => "production",
        /// The company that released it.
        Distributor => "distributor",
        /// The studio that owns it.
        Studio => "studio",
    }
}

stored_enum! {
    /// A person's part in a title, as `credit.kind` stores it.
    CreditKind {
        /// A billed performer.
        Cast => "cast",
        /// A member of the crew.
        Crew => "crew",
        /// A performer credited for one episode.
        Guest => "guest",
    }
}

stored_enum! {
    /// An episode's place in its season, as `episode.episode_type` stores it.
    EpisodeType {
        /// An ordinary episode.
        Standard => "standard",
        /// A special.
        Special => "special",
        /// The last episode of a season or series.
        Finale => "finale",
        /// The episode before a mid-season break.
        MidSeason => "mid_season",
        /// The first episode of a season.
        Premiere => "premiere",
    }
}

stored_enum! {
    /// An alternate episode numbering, as `episode_ordering.scheme` stores it.
    ///
    /// `Aired` is the canonical order the `episode` columns already hold, so an overlay row
    /// with it is redundant but not illegal; the other three are the reason the table exists.
    OrderingScheme {
        /// The canonical order, as published by the primary provider.
        Aired => "aired",
        /// The disc order.
        Dvd => "dvd",
        /// A single running number across every season.
        Absolute => "absolute",
        /// The anime broadcast order, which often differs from the disc order.
        Anime => "anime",
    }
}

/// A season's position within a title; `0` is the specials season.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SeasonNumber(u32);

impl SeasonNumber {
    /// Wraps the number the database stores.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The stored number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<i64> for SeasonNumber {
    type Error = CatalogueError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value)
            .map(Self)
            .map_err(|_| CatalogueError::NotASeasonNumber { value })
    }
}

/// An episode's position within its season; `0` is the specials slot some providers use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EpisodeNumber(u32);

impl EpisodeNumber {
    /// Wraps the number the database stores.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The stored number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<i64> for EpisodeNumber {
    type Error = CatalogueError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value)
            .map(Self)
            .map_err(|_| CatalogueError::NotAnEpisodeNumber { value })
    }
}

/// Why a catalogue value was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CatalogueError {
    /// The value does not fit a season number, which is unsigned because a negative season is
    /// not a season.
    #[error("`{value}` is not a season number; it must be zero or greater and fit in 32 bits")]
    NotASeasonNumber {
        /// The value that was refused.
        value: i64,
    },
    /// The same for an episode number.
    #[error("`{value}` is not an episode number; it must be zero or greater and fit in 32 bits")]
    NotAnEpisodeNumber {
        /// The value that was refused.
        value: i64,
    },
}

/// Where an episode sits in a title: the pair every ordering scheme sorts by.
///
/// The derived [`Ord`] is the aired order — season first, then episode — because that is the
/// order the canonical `episode` columns describe. A scheme that wants another one renumbers
/// through [`EpisodeOrdering`] instead of changing this comparison, which is what keeps two
/// orderings from disagreeing about the same episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EpisodePosition {
    /// The season the episode belongs to; `0` is the specials season.
    pub season_number: SeasonNumber,
    /// The episode's number inside that season.
    pub episode_number: EpisodeNumber,
}

/// One row of `title`: a film or a series, with the denormalized artwork the browse grid
/// reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Title {
    /// The row id.
    pub id: i64,
    /// Film or series.
    pub kind: TitleKind,
    /// Ordinary, anime, documentary, or reality.
    pub subtype: TitleSubtype,
    /// The name to display.
    pub name: NonEmptyText,
    /// The name in the original language, when it differs.
    pub original_name: Option<String>,
    /// The name to sort by, which is not the display name for a leading article.
    pub sort_name: NonEmptyText,
    /// The synopsis.
    pub overview: Option<String>,
    /// The marketing line.
    pub tagline: Option<String>,
    /// Where production stands.
    pub status: Option<TitleStatus>,
    /// The cinema release date, for a film.
    pub release_date: Option<Date>,
    /// The first air date, for a series.
    pub first_air_date: Option<Date>,
    /// The most recent air date, for a series.
    pub last_air_date: Option<Date>,
    /// The runtime of a film, in minutes.
    pub runtime_minutes: Option<u32>,
    /// The typical runtime of an episode, in minutes.
    pub episode_runtime_minutes: Option<u32>,
    /// The original language, as a BCP-47 tag.
    pub original_language: Option<String>,
    /// The production countries, as ISO-3166 alpha-2 codes.
    // The column is a JSON array; the repository owns the encoding, the domain keeps the list.
    pub origin_country: Option<Vec<String>>,
    /// The certification the title carries in its home market.
    pub content_rating: Option<String>,
    /// The `IMDb` rating, out of ten.
    pub imdb_rating: Option<f64>,
    /// How many votes the `IMDb` rating has.
    pub imdb_votes: Option<u64>,
    /// The `TMDB` rating, out of ten.
    pub tmdb_rating: Option<f64>,
    /// How many votes the `TMDB` rating has.
    pub tmdb_votes: Option<u64>,
    /// The provider's popularity score.
    pub popularity: Option<f64>,
    /// Whether the title is flagged adult by the provider.
    pub adult: bool,
    /// Whether production is ongoing.
    pub in_production: bool,
    /// How many seasons the title has, when known.
    pub season_count: Option<u32>,
    /// How many episodes the title has, when known.
    pub episode_count: Option<u32>,
    /// The provider that owns the primary poster path.
    pub poster_provider: Option<String>,
    /// The primary poster path, relative to its provider.
    pub poster_path: Option<String>,
    /// The primary poster's blurhash.
    pub poster_blurhash: Option<String>,
    /// The provider that owns the primary backdrop path.
    pub backdrop_provider: Option<String>,
    /// The primary backdrop path, relative to its provider.
    pub backdrop_path: Option<String>,
    /// The primary backdrop's blurhash.
    pub backdrop_blurhash: Option<String>,
    /// The logo path, relative to its provider.
    pub logo_path: Option<String>,
    /// The site a trailer is hosted on.
    pub trailer_site: Option<String>,
    /// The trailer's key on that site.
    pub trailer_key: Option<String>,
    /// The trailer's language, as a BCP-47 tag.
    pub trailer_language: Option<String>,
    /// When the trailer was published.
    pub trailer_published_at: Option<Timestamp>,
    /// The title's own web page.
    pub homepage: Option<String>,
    /// How complete the metadata is.
    pub metadata_state: MetadataState,
    /// When the metadata was last successfully synced.
    pub last_synced_at: Option<Timestamp>,
    /// When the row was created.
    pub created_at: Timestamp,
    /// When the row was last written.
    pub updated_at: Timestamp,
}

/// One row of `title_alias`: another name the title is known by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleAlias {
    /// The row id.
    pub id: i64,
    /// The title the alias belongs to.
    pub title_id: i64,
    /// The alias itself.
    pub alias: NonEmptyText,
    /// The market the name is used in.
    pub region: Option<String>,
    /// Alternative, working, or original.
    pub kind: AliasKind,
}

/// One row of `genre`: a name a title can be tagged with.
///
/// The name is the whole entity, which is why the schema makes it unique and why an empty one
/// is not representable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genre {
    /// The row id.
    pub id: i64,
    /// The genre's name.
    pub name: NonEmptyText,
}

/// One row of `keyword`: a free-text descriptor attached to a title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyword {
    /// The row id.
    pub id: i64,
    /// The keyword itself.
    pub name: NonEmptyText,
}

/// One row of `company`: a studio, network, or distributor.
///
/// A company is unique by `(name, provider)` in the schema, so `provider` and `external_id`
/// are how the same studio found through two providers stays one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Company {
    /// The row id.
    pub id: i64,
    /// The company's name.
    pub name: NonEmptyText,
    /// The logo path, relative to its provider.
    pub logo_path: Option<String>,
    /// The provider the company was discovered through.
    pub provider: Option<String>,
    /// The company's id at that provider.
    pub external_id: Option<String>,
}

/// One row of `person`: someone credited on a title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// The row id.
    pub id: i64,
    /// The person's display name.
    pub name: NonEmptyText,
    /// The name to sort by.
    pub sort_name: NonEmptyText,
    /// The department the person is best known for.
    pub known_for_department: Option<String>,
    /// The provider that owns the profile path.
    pub profile_provider: Option<String>,
    /// The profile image path, relative to its provider.
    pub profile_path: Option<String>,
    /// The profile image's blurhash.
    pub profile_blurhash: Option<String>,
    /// When the row was last written, when it ever has been.
    pub updated_at: Option<Timestamp>,
}

/// One row of `credit`: a person's part in a title, or in one episode of it.
///
/// The schema's uniqueness key is `(title_id, person_id, kind, job, character)`, so the role
/// text is part of the identity rather than a label: a person can be both director and
/// writer, and playing two characters is two credits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    /// The row id.
    pub id: i64,
    /// The title the credit is for.
    pub title_id: i64,
    /// The episode, for a guest credit or an episode-level crew credit.
    pub episode_id: Option<i64>,
    /// The person credited.
    pub person_id: i64,
    /// Cast, crew, or guest.
    pub kind: CreditKind,
    /// The character played, for a cast or guest credit.
    pub character: Option<String>,
    /// The job held, for a crew credit.
    pub job: Option<String>,
    /// The department the job belongs to.
    pub department: Option<String>,
    /// The billing position within its `(title_id, kind)` group.
    pub order_index: Option<u32>,
    /// How many episodes the person appears in, for a recurring credit.
    pub episode_count: Option<u32>,
}

/// One row of `season`: a season of a series, including the specials season.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Season {
    /// The row id.
    pub id: i64,
    /// The series the season belongs to.
    pub title_id: i64,
    /// The season's number; `0` is the specials season.
    pub season_number: SeasonNumber,
    /// The season's name, when it has one.
    pub name: Option<String>,
    /// The season's synopsis.
    pub overview: Option<String>,
    /// The date the season first aired.
    pub air_date: Option<Date>,
    /// The provider that owns the poster path.
    pub poster_provider: Option<String>,
    /// The poster path, relative to its provider.
    pub poster_path: Option<String>,
    /// The poster's blurhash.
    pub poster_blurhash: Option<String>,
    /// How many episodes the season has, when known.
    pub episode_count: Option<u32>,
    /// Whether this is the specials season.
    pub is_specials: bool,
    /// When the row was last written.
    pub updated_at: Timestamp,
}

/// One row of `episode`: an episode in its canonical, aired numbering.
///
/// The canonical numbers live here and are never rewritten; an alternate order is an
/// [`EpisodeOrdering`] row beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    /// The row id.
    pub id: i64,
    /// The series the episode belongs to.
    pub title_id: i64,
    /// The season the episode belongs to.
    pub season_id: i64,
    /// The season's number, denormalized from `season`.
    pub season_number: SeasonNumber,
    /// The episode's number inside the season.
    pub episode_number: EpisodeNumber,
    /// The episode's number in a single running sequence, for anime.
    pub absolute_number: Option<EpisodeNumber>,
    /// The episode's name.
    pub name: Option<String>,
    /// The episode's synopsis.
    pub overview: Option<String>,
    /// The date the episode first aired.
    pub air_date: Option<Date>,
    /// The episode's runtime, in minutes.
    pub runtime_minutes: Option<u32>,
    /// The provider that owns the still path.
    pub still_provider: Option<String>,
    /// The still image path, relative to its provider.
    pub still_path: Option<String>,
    /// The still's blurhash.
    pub still_blurhash: Option<String>,
    /// The provider's average vote, out of ten.
    pub vote_average: Option<f64>,
    /// How many votes the average has.
    pub vote_count: Option<u64>,
    /// Standard, special, finale, mid-season, or premiere.
    pub episode_type: Option<EpisodeType>,
    /// The production code the studio printed on it.
    pub production_code: Option<String>,
    /// When the row was last written.
    pub updated_at: Timestamp,
}

impl Episode {
    /// Where the episode sits in the canonical, aired order.
    #[must_use]
    pub const fn position(&self) -> EpisodePosition {
        EpisodePosition {
            season_number: self.season_number,
            episode_number: self.episode_number,
        }
    }
}

/// One row of `episode_ordering`: an alternate numbering of an episode.
///
/// Overlays are additive by design: the canonical `episode` numbers stay put so that files
/// already matched to them keep matching after the user switches scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EpisodeOrdering {
    /// The episode this numbering applies to.
    pub episode_id: i64,
    /// Which alternate numbering this is.
    pub scheme: OrderingScheme,
    /// The episode's season number under that scheme.
    pub season_number: SeasonNumber,
    /// The episode's episode number under that scheme.
    pub episode_number: EpisodeNumber,
}

impl EpisodeOrdering {
    /// Where the episode sits under this scheme.
    #[must_use]
    pub const fn position(self) -> EpisodePosition {
        EpisodePosition {
            season_number: self.season_number,
            episode_number: self.episode_number,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Pins each enum's spellings and their order.
    ///
    /// The expected values are written out here rather than read from the schema, because this
    /// crate may not touch a database (AGENTS R6) — so this test catches an accidental
    /// variant rename, a dropped variant, or a reordering of `ALL`, and nothing more. The
    /// list must agree with `DATA_MODEL` §2 *and* with the migration; `peregrine-storage`
    /// asserts the second half against the live `CHECK` clause, and a change to either should
    /// fail both.
    macro_rules! assert_check_values {
        ($enum:ty, [$($value:literal),+ $(,)?]) => {
            assert_eq!(
                <$enum>::ALL.iter().map(|value| value.as_str()).collect::<Vec<_>>(),
                [$($value),+],
                "{} no longer lists the values this test pins",
                stringify!($enum)
            );
        };
    }

    macro_rules! assert_text_round_trip {
        ($enum:ty) => {
            for value in <$enum>::ALL {
                assert_eq!(<$enum>::parse(value.as_str()), Some(*value));
                let json = serde_json::to_string(value).expect("serializes");
                assert_eq!(json, format!("\"{}\"", value.as_str()));
                assert_eq!(
                    serde_json::from_str::<$enum>(&json).expect("deserializes"),
                    *value
                );
            }
        };
    }

    fn date(year: i16, month: i8, day: i8) -> Date {
        Date::new(year, month, day).expect("a real calendar date")
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("the fixture text is not empty")
    }

    fn title() -> Title {
        Title {
            id: 1,
            kind: TitleKind::Movie,
            subtype: TitleSubtype::Standard,
            name: text("Blade Runner 2049"),
            original_name: Some("Blade Runner 2049".to_owned()),
            sort_name: text("Blade Runner 2049"),
            overview: Some("A blade runner uncovers a secret.".to_owned()),
            tagline: Some("There is an order to things.".to_owned()),
            status: Some(TitleStatus::Released),
            release_date: Some(date(2017, 10, 6)),
            first_air_date: None,
            last_air_date: None,
            runtime_minutes: Some(164),
            episode_runtime_minutes: None,
            original_language: Some("en".to_owned()),
            origin_country: Some(vec!["US".to_owned(), "GB".to_owned()]),
            content_rating: Some("R".to_owned()),
            imdb_rating: Some(8.0),
            imdb_votes: Some(600_000),
            tmdb_rating: Some(7.6),
            tmdb_votes: Some(12_000),
            popularity: Some(42.5),
            adult: false,
            in_production: false,
            season_count: None,
            episode_count: None,
            poster_provider: Some("tmdb".to_owned()),
            poster_path: Some("/poster.jpg".to_owned()),
            poster_blurhash: Some("L6PZfSi_.AyE".to_owned()),
            backdrop_provider: Some("tmdb".to_owned()),
            backdrop_path: Some("/backdrop.jpg".to_owned()),
            backdrop_blurhash: None,
            logo_path: Some("/logo.png".to_owned()),
            trailer_site: Some("YouTube".to_owned()),
            trailer_key: Some("gCcx85zbxz4".to_owned()),
            trailer_language: Some("en".to_owned()),
            trailer_published_at: Some(at(1_500_000_000_000)),
            homepage: Some("https://example.invalid/".to_owned()),
            metadata_state: MetadataState::Complete,
            last_synced_at: Some(at(1_700_000_000_000)),
            created_at: at(1_600_000_000_000),
            updated_at: at(1_700_000_000_000),
        }
    }

    fn episode() -> Episode {
        Episode {
            id: 10,
            title_id: 2,
            season_id: 3,
            season_number: SeasonNumber::new(1),
            episode_number: EpisodeNumber::new(2),
            absolute_number: Some(EpisodeNumber::new(2)),
            name: Some("The One With The Episode".to_owned()),
            overview: None,
            air_date: Some(date(2024, 3, 4)),
            runtime_minutes: Some(48),
            still_provider: Some("tmdb".to_owned()),
            still_path: Some("/still.jpg".to_owned()),
            still_blurhash: None,
            vote_average: Some(8.1),
            vote_count: Some(120),
            episode_type: Some(EpisodeType::Standard),
            production_code: Some("101".to_owned()),
            updated_at: at(1_700_000_000_000),
        }
    }

    #[test]
    fn every_enum_pins_its_spellings_and_their_order() {
        assert_check_values!(TitleKind, ["movie", "show"]);
        assert_check_values!(
            TitleSubtype,
            ["standard", "anime", "documentary", "reality"]
        );
        assert_check_values!(
            TitleStatus,
            [
                "rumored",
                "planned",
                "in_production",
                "returning",
                "ended",
                "canceled",
                "released",
                "upcoming"
            ]
        );
        assert_check_values!(
            MetadataState,
            ["stub", "partial", "complete", "stale", "failed"]
        );
        assert_check_values!(AliasKind, ["alternative", "working", "original"]);
        assert_check_values!(
            CompanyRole,
            ["network", "production", "distributor", "studio"]
        );
        assert_check_values!(CreditKind, ["cast", "crew", "guest"]);
        assert_check_values!(
            EpisodeType,
            ["standard", "special", "finale", "mid_season", "premiere"]
        );
        assert_check_values!(OrderingScheme, ["aired", "dvd", "absolute", "anime"]);
    }

    #[test]
    fn every_enum_round_trips_through_its_stored_text_and_json() {
        assert_text_round_trip!(TitleKind);
        assert_text_round_trip!(TitleSubtype);
        assert_text_round_trip!(TitleStatus);
        assert_text_round_trip!(MetadataState);
        assert_text_round_trip!(AliasKind);
        assert_text_round_trip!(CompanyRole);
        assert_text_round_trip!(CreditKind);
        assert_text_round_trip!(EpisodeType);
        assert_text_round_trip!(OrderingScheme);
    }

    #[test]
    fn an_enum_refuses_a_value_outside_its_constraint() {
        assert_eq!(TitleKind::parse("documentary"), None);
        assert_eq!(TitleKind::parse(""), None);
        assert_eq!(MetadataState::parse("fresh"), None);
        assert!(serde_json::from_str::<TitleKind>("\"documentary\"").is_err());
        assert!(
            serde_json::from_str::<TitleStatus>("\"in production\"").is_err(),
            "the stored spelling is the one with an underscore"
        );
    }

    #[test]
    fn season_and_episode_numbers_refuse_anything_the_column_cannot_hold() {
        assert_eq!(SeasonNumber::try_from(0).unwrap().get(), 0);
        assert_eq!(
            SeasonNumber::try_from(i64::from(u32::MAX)).unwrap().get(),
            u32::MAX
        );
        assert_eq!(EpisodeNumber::try_from(1).unwrap().get(), 1);

        for refused in [-1, i64::MIN] {
            assert_eq!(
                SeasonNumber::try_from(refused),
                Err(CatalogueError::NotASeasonNumber { value: refused })
            );
            assert_eq!(
                EpisodeNumber::try_from(refused),
                Err(CatalogueError::NotAnEpisodeNumber { value: refused })
            );
        }
        assert_eq!(
            SeasonNumber::try_from(i64::from(u32::MAX) + 1),
            Err(CatalogueError::NotASeasonNumber {
                value: i64::from(u32::MAX) + 1
            })
        );
    }

    #[test]
    fn a_number_serializes_as_a_bare_integer() {
        assert_eq!(serde_json::to_string(&SeasonNumber::new(2)).unwrap(), "2");
        assert_eq!(
            serde_json::from_str::<EpisodeNumber>("13").unwrap(),
            EpisodeNumber::new(13)
        );
        assert!(
            serde_json::from_str::<SeasonNumber>("-1").is_err(),
            "the newtype refuses a negative number on the way in as well"
        );
    }

    #[test]
    fn episode_positions_order_by_season_then_episode() {
        let mut positions = [
            EpisodePosition {
                season_number: SeasonNumber::new(2),
                episode_number: EpisodeNumber::new(1),
            },
            EpisodePosition {
                season_number: SeasonNumber::new(1),
                episode_number: EpisodeNumber::new(9),
            },
            EpisodePosition {
                season_number: SeasonNumber::new(1),
                episode_number: EpisodeNumber::new(10),
            },
            EpisodePosition {
                season_number: SeasonNumber::new(0),
                episode_number: EpisodeNumber::new(1),
            },
        ];
        positions.sort();
        let ordered: Vec<(u32, u32)> = positions
            .iter()
            .map(|position| (position.season_number.get(), position.episode_number.get()))
            .collect();
        assert_eq!(ordered, [(0, 1), (1, 9), (1, 10), (2, 1)]);
    }

    #[test]
    fn an_episode_reports_its_canonical_position() {
        let episode = episode();
        assert_eq!(
            episode.position(),
            EpisodePosition {
                season_number: SeasonNumber::new(1),
                episode_number: EpisodeNumber::new(2),
            }
        );

        let overlay = EpisodeOrdering {
            episode_id: episode.id,
            scheme: OrderingScheme::Absolute,
            season_number: SeasonNumber::new(1),
            episode_number: EpisodeNumber::new(26),
        };
        assert_eq!(overlay.position().episode_number, EpisodeNumber::new(26));
        assert_eq!(overlay.scheme, OrderingScheme::Absolute);
    }

    #[test]
    fn a_title_round_trips_through_json() {
        let title = title();
        let json = serde_json::to_string(&title).unwrap();
        assert_eq!(serde_json::from_str::<Title>(&json).unwrap(), title);
    }

    #[test]
    fn a_title_json_rejects_an_unknown_kind_and_an_empty_name() {
        let json = serde_json::to_value(title()).unwrap();
        let mut unknown_kind = json.clone();
        unknown_kind["kind"] = serde_json::Value::String("documentary".to_owned());
        assert!(serde_json::from_value::<Title>(unknown_kind).is_err());

        let mut empty_name = json.clone();
        empty_name["name"] = serde_json::Value::String(String::new());
        assert!(serde_json::from_value::<Title>(empty_name).is_err());

        let mut negative_count = json;
        negative_count["season_count"] = serde_json::Value::from(-1);
        assert!(serde_json::from_value::<Title>(negative_count).is_err());
    }

    #[test]
    fn a_title_json_requires_every_not_null_column() {
        let mut json = serde_json::to_value(title()).unwrap();
        json.as_object_mut().unwrap().remove("adult");
        assert!(serde_json::from_value::<Title>(json).is_err());

        let mut missing_name = serde_json::to_value(title()).unwrap();
        missing_name.as_object_mut().unwrap().remove("sort_name");
        assert!(serde_json::from_value::<Title>(missing_name).is_err());
    }

    #[test]
    fn a_title_json_ignores_a_column_this_build_does_not_know() {
        // The entities are read back from rows written by whichever build wrote them, so an
        // unknown field has to be a newer column rather than a hard failure.
        let mut json = serde_json::to_value(title()).unwrap();
        json["written_by_a_newer_build"] = serde_json::Value::from(1);
        assert_eq!(serde_json::from_value::<Title>(json).unwrap(), title());
    }

    #[test]
    fn the_remaining_catalogue_entities_round_trip_through_json() {
        let alias = TitleAlias {
            id: 2,
            title_id: 1,
            alias: text("Dune: Part Two"),
            region: Some("US".to_owned()),
            kind: AliasKind::Alternative,
        };
        assert_eq!(round_trip(&alias), alias);

        let genre = Genre {
            id: 3,
            name: text("Science Fiction"),
        };
        assert_eq!(round_trip(&genre), genre);

        let keyword = Keyword {
            id: 4,
            name: text("dystopia"),
        };
        assert_eq!(round_trip(&keyword), keyword);

        let company = Company {
            id: 5,
            name: text("Alcon Entertainment"),
            logo_path: Some("/logo.png".to_owned()),
            provider: Some("tmdb".to_owned()),
            external_id: Some("1088".to_owned()),
        };
        assert_eq!(round_trip(&company), company);

        let person = Person {
            id: 6,
            name: text("Denis Villeneuve"),
            sort_name: text("Villeneuve, Denis"),
            known_for_department: Some("Directing".to_owned()),
            profile_provider: Some("tmdb".to_owned()),
            profile_path: None,
            profile_blurhash: None,
            updated_at: Some(at(1_700_000_000_000)),
        };
        assert_eq!(round_trip(&person), person);

        let credit = Credit {
            id: 7,
            title_id: 1,
            episode_id: None,
            person_id: 6,
            kind: CreditKind::Crew,
            character: None,
            job: Some("Director".to_owned()),
            department: Some("Directing".to_owned()),
            order_index: Some(0),
            episode_count: None,
        };
        assert_eq!(round_trip(&credit), credit);

        let season = Season {
            id: 8,
            title_id: 1,
            season_number: SeasonNumber::new(1),
            name: Some("Season 1".to_owned()),
            overview: None,
            air_date: Some(date(2020, 1, 1)),
            poster_provider: None,
            poster_path: None,
            poster_blurhash: None,
            episode_count: Some(10),
            is_specials: false,
            updated_at: at(1_700_000_000_000),
        };
        assert_eq!(round_trip(&season), season);

        let episode = episode();
        assert_eq!(round_trip(&episode), episode);

        let ordering = EpisodeOrdering {
            episode_id: episode.id,
            scheme: OrderingScheme::Dvd,
            season_number: SeasonNumber::new(1),
            episode_number: EpisodeNumber::new(3),
        };
        assert_eq!(round_trip(&ordering), ordering);
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serializes");
        serde_json::from_str(&json).expect("deserializes")
    }

    #[test]
    fn a_genre_without_a_name_is_not_representable() {
        assert!(
            serde_json::from_str::<Genre>("{\"id\":1,\"name\":\"\"}").is_err(),
            "the name is the entity"
        );
    }

    proptest! {
        #[test]
        fn a_position_orders_by_season_then_episode(
            first_season in 0u32..40,
            first_episode in 0u32..40,
            second_season in 0u32..40,
            second_episode in 0u32..40,
        ) {
            let first = EpisodePosition {
                season_number: SeasonNumber::new(first_season),
                episode_number: EpisodeNumber::new(first_episode),
            };
            let second = EpisodePosition {
                season_number: SeasonNumber::new(second_season),
                episode_number: EpisodeNumber::new(second_episode),
            };
            let expected = (first_season, first_episode).cmp(&(second_season, second_episode));
            prop_assert_eq!(first.cmp(&second), expected);
        }

        #[test]
        fn a_number_conversion_never_panics(value in any::<i64>()) {
            let _ = SeasonNumber::try_from(value);
            let _ = EpisodeNumber::try_from(value);
        }

        #[test]
        fn a_number_round_trips_through_json(value in 0u32..1_000_000) {
            let season = SeasonNumber::new(value);
            let json = serde_json::to_string(&season).expect("serializes");
            prop_assert_eq!(serde_json::from_str::<SeasonNumber>(&json).expect("deserializes"), season);
        }
    }
}
