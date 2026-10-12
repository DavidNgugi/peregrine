//! A metadata provider's declared capabilities (docs/ARCHITECTURE.md §13).
//!
//! The set is what an adapter publishes about itself and what every caller reads before it
//! asks for anything: the registry lists it (PG-E5-12), the diagnostics screen shows it
//! (PG-E5-22), and a call for something the provider never claimed it could serve is refused
//! rather than spending a rate-limit token to find out.
//!
//! The names are `ARCHITECTURE` §13's eight. Declaring one is a claim about the *provider*,
//! not about this build: `OMDb` is a ratings fallback and TVDB is optional, so a provider that
//! declares three of the eight is the normal case rather than an incomplete one, and a port
//! that could only be asked for what every provider supports would be a port that cannot
//! express the difference. The port's *operations* are a subset of these today — `search` is
//! the one whose request and response the flows already fix — and each remaining operation
//! arrives with the normalized type it returns (PG-E5-06 to PG-E5-16).

/// One thing a provider can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Capability {
    /// Find titles by name.
    Search,
    /// Read one title's full record.
    Title,
    /// Read one season and its episodes.
    Season,
    /// Read one episode.
    Episode,
    /// Read what is popular now.
    Trending,
    /// List what changed at the provider since a cursor.
    Changes,
    /// Read artwork.
    Images,
    /// Read trailers and other videos.
    Videos,
}

impl Capability {
    /// Every capability, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Search,
        Self::Title,
        Self::Season,
        Self::Episode,
        Self::Trending,
        Self::Changes,
        Self::Images,
        Self::Videos,
    ];

    /// The provider's own name for it, which is what a log line, the diagnostics screen, and
    /// a refusal all show. The spellings are `ARCHITECTURE` §13's.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Title => "get_title",
            Self::Season => "get_season",
            Self::Episode => "get_episode",
            Self::Trending => "trending",
            Self::Changes => "changes",
            Self::Images => "images",
            Self::Videos => "videos",
        }
    }

    /// The bit this capability owns in [`Capabilities`].
    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// The set of capabilities one provider declares.
///
/// A `u8` rather than a `BTreeSet`: the set is small, fixed, and `Copy`, and copying it
/// costs nothing where a set would allocate on every descriptor read. It is a newtype rather
/// than a bare integer because the bits are an implementation detail — nothing outside this
/// module may name one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Capabilities(u8);

impl Capabilities {
    /// A provider that can be asked nothing, which no registered provider may be.
    pub const EMPTY: Self = Self(0);

    /// Every capability this build knows.
    #[must_use]
    pub const fn all() -> Self {
        let mut bits = 0_u8;
        let mut index = 0;
        while index < Capability::ALL.len() {
            bits |= Capability::ALL[index].bit();
            index += 1;
        }
        Self(bits)
    }

    /// The set with `capability` added.
    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.bit())
    }

    /// Whether `capability` is declared.
    #[must_use]
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    /// Whether nothing is declared.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The declared capabilities, in declaration order.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |one| self.contains(*one))
    }
}

impl FromIterator<Capability> for Capabilities {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        iter.into_iter().fold(Self::EMPTY, Self::with)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_contains_exactly_what_was_added_to_it() {
        let declared = Capabilities::EMPTY.with(Capability::Search);

        assert!(declared.contains(Capability::Search));
        assert!(!declared.contains(Capability::Changes));
        assert!(!declared.is_empty());
    }

    #[test]
    fn the_empty_set_contains_nothing() {
        assert!(Capabilities::EMPTY.is_empty());
        for capability in Capability::ALL {
            assert!(!Capabilities::EMPTY.contains(capability));
        }
    }

    #[test]
    fn the_full_set_is_every_capability_and_no_more() {
        let all = Capabilities::all();

        for capability in Capability::ALL {
            assert!(all.contains(capability), "{} missing", capability.as_str());
        }
        assert_eq!(all.iter().count(), Capability::ALL.len());
    }

    #[test]
    fn adding_a_capability_twice_is_adding_it_once() {
        let once = Capabilities::EMPTY.with(Capability::Changes);
        let twice = once.with(Capability::Changes);

        assert_eq!(once, twice);
    }

    #[test]
    fn iterating_yields_the_declared_ones_in_declaration_order() {
        let declared = Capabilities::EMPTY
            .with(Capability::Videos)
            .with(Capability::Search);

        let named: Vec<&str> = declared.iter().map(Capability::as_str).collect();
        assert_eq!(named, ["search", "videos"]);
    }

    #[test]
    fn a_set_can_be_collected_from_a_list() {
        let declared: Capabilities = [Capability::Videos, Capability::Search]
            .into_iter()
            .collect();

        assert!(!declared.contains(Capability::Changes));
        assert_eq!(declared.iter().count(), 2);
    }

    #[test]
    fn every_capability_has_a_name_and_a_bit_of_its_own() {
        // A duplicate name would make two capabilities indistinguishable in a log line or a
        // refusal; a duplicate bit would make one silently stand for the other. A `u8` holds
        // eight, so the list growing past that is a compile error at `bit` rather than a
        // silent collision — this asserts the eight that are there.
        let mut names = Vec::new();
        let mut bits = Vec::new();
        for capability in Capability::ALL {
            names.push(capability.as_str());
            bits.push(capability.bit());
        }
        names.sort_unstable();
        names.dedup();
        bits.sort_unstable();
        bits.dedup();

        assert_eq!(names.len(), Capability::ALL.len());
        assert_eq!(bits.len(), Capability::ALL.len());
        assert!(bits.iter().all(|bit| *bit != 0), "a capability owns no bit");
    }

    #[test]
    fn the_names_are_the_ones_the_architecture_lists() {
        // ARCHITECTURE §13 names all eight, and the diagnostics screen and every refusal
        // quote them, so a rename here is a documentation change as well as a code one.
        let named: Vec<&str> = Capability::ALL.iter().map(|one| one.as_str()).collect();

        assert_eq!(
            named,
            [
                "search",
                "get_title",
                "get_season",
                "get_episode",
                "trending",
                "changes",
                "images",
                "videos",
            ]
        );
    }
}
