//! What a provider is, what it can be asked for, and the terms it answers under.
//!
//! [ADR 0007](../../../docs/adr/0007-metadata-sources.md) rule 3 makes this a requirement
//! rather than documentation: "provider results are normalized into domain types with
//! explicit capability declarations, rate limits, cache policy, and attribution
//! requirements". Each of those is a field here, and each is validated on construction, so a
//! descriptor that a caller can hold is one a caller can trust.
//!
//! The numbers themselves are **not** here. This module defines the shape; an adapter fills
//! it from [research/data-sources.md](../../../docs/research/data-sources.md), which is the
//! document that says what each provider actually allows and marks the unverified figures as
//! unverified. Hard-coding a rate limit in the port would put a researched number and a
//! remembered one in the same place with nothing to tell them apart.

use std::num::NonZeroU32;
use std::time::Duration;

use thiserror::Error;

use peregrine_domain::id::Provider;

use crate::capability::Capabilities;

/// How often a provider may be asked, and what happens when it says no.
///
/// This is the declaration the token bucket is built from (PG-E5-03), so it has to be
/// complete enough to build one: a sustained rate, a burst, and whether the provider tells us
/// how long to wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatePolicy {
    requests: NonZeroU32,
    window: Duration,
    burst: NonZeroU32,
    honours_retry_after: bool,
}

impl RatePolicy {
    /// Declares a policy of `requests` per `window`, allowing `burst` to start at once.
    ///
    /// # Errors
    ///
    /// [`DescriptorError::EmptyRateWindow`] when the window is zero, which would be a
    /// division by it rather than a rate.
    pub fn new(
        requests: NonZeroU32,
        window: Duration,
        burst: NonZeroU32,
        honours_retry_after: bool,
    ) -> Result<Self, DescriptorError> {
        if window.is_zero() {
            return Err(DescriptorError::EmptyRateWindow);
        }
        Ok(Self {
            requests,
            window,
            burst,
            honours_retry_after,
        })
    }

    /// Requests allowed per window.
    #[must_use]
    pub const fn requests(&self) -> NonZeroU32 {
        self.requests
    }

    /// The window they are allowed in.
    #[must_use]
    pub const fn window(&self) -> Duration {
        self.window
    }

    /// Requests allowed to start together, before the sustained rate applies.
    #[must_use]
    pub const fn burst(&self) -> NonZeroU32 {
        self.burst
    }

    /// Whether the provider sends `Retry-After`, and therefore whether a `429` carries a
    /// delay worth honouring over the bucket's own backoff.
    #[must_use]
    pub const fn honours_retry_after(&self) -> bool {
        self.honours_retry_after
    }
}

/// How long an answer may be served, and whether it can be revalidated cheaply.
///
/// The ceiling is separate from the freshness because they come from different places:
/// freshness is a judgement about how fast the data changes, and the ceiling is a term of
/// service. TMDB's is six months and the ADR calls refreshing or purging at or before it "a
/// compliance requirement, not a nicety" (ADR 0007 rule 9), so the two cannot be one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    max_age: Duration,
    ceiling: Option<Duration>,
    revalidates: bool,
}

impl CachePolicy {
    /// Declares how long an answer stays fresh, the provider's own ceiling if it has one, and
    /// whether the endpoint answers conditional requests.
    ///
    /// # Errors
    ///
    /// [`DescriptorError::CeilingBelowFreshness`] when the ceiling is shorter than the
    /// freshness: that descriptor would say "serve this for a week" and "never keep it longer
    /// than a day" at the same time, and whichever of the two a caller read first would win.
    pub fn new(
        max_age: Duration,
        ceiling: Option<Duration>,
        revalidates: bool,
    ) -> Result<Self, DescriptorError> {
        if let Some(ceiling) = ceiling
            && ceiling < max_age
        {
            return Err(DescriptorError::CeilingBelowFreshness { max_age, ceiling });
        }
        Ok(Self {
            max_age,
            ceiling,
            revalidates,
        })
    }

    /// How long an answer may be served without asking again.
    #[must_use]
    pub const fn max_age(&self) -> Duration {
        self.max_age
    }

    /// The longest the provider's terms allow its content to be kept, when they say.
    #[must_use]
    pub const fn ceiling(&self) -> Option<Duration> {
        self.ceiling
    }

    /// Whether the endpoint answers `ETag` or `Last-Modified`, and a repeat call can therefore
    /// transfer no body (PG-E5-04).
    #[must_use]
    pub const fn revalidates(&self) -> bool {
        self.revalidates
    }
}

/// What must be shown wherever a provider's data is displayed.
///
/// ADR 0007 rule 4 makes this a first-class requirement: TMDB requires its logo to identify
/// the use of its APIs and a notice reproduced verbatim, and "any other provider requiring a
/// notice gets one". The notice is `&'static str` because it is a fixed string the provider's
/// terms fix — a notice assembled at runtime is one that can be edited by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attribution {
    notice: &'static str,
    requires_logo: bool,
}

impl Attribution {
    /// Declares a notice and whether the provider's logo is required alongside it.
    ///
    /// # Errors
    ///
    /// [`DescriptorError::EmptyNotice`] when there is no notice: a provider that requires
    /// attribution and names none is one this build cannot comply with, and finding that out
    /// at release is worse than finding it out here.
    pub fn new(notice: &'static str, requires_logo: bool) -> Result<Self, DescriptorError> {
        if notice.trim().is_empty() {
            return Err(DescriptorError::EmptyNotice);
        }
        Ok(Self {
            notice,
            requires_logo,
        })
    }

    /// The notice, verbatim.
    #[must_use]
    pub const fn notice(&self) -> &'static str {
        self.notice
    }

    /// Whether the logo must be shown as well.
    #[must_use]
    pub const fn requires_logo(&self) -> bool {
        self.requires_logo
    }
}

/// Everything a caller needs to know about a provider before it asks for anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    provider: Provider,
    display_name: &'static str,
    capabilities: Capabilities,
    rate: RatePolicy,
    cache: CachePolicy,
    attribution: Option<Attribution>,
}

impl ProviderDescriptor {
    /// Declares a provider.
    ///
    /// # Errors
    ///
    /// [`DescriptorError::NoCapabilities`] when nothing is declared — a registered provider
    /// that can be asked nothing is a row in the settings list that does nothing, and ADR 0007
    /// rule 5 already establishes that a provider may instead be *disabled*; and
    /// [`DescriptorError::EmptyDisplayName`], because the settings list and the diagnostics
    /// screen both have to name it.
    pub fn new(
        provider: Provider,
        display_name: &'static str,
        capabilities: Capabilities,
        rate: RatePolicy,
        cache: CachePolicy,
        attribution: Option<Attribution>,
    ) -> Result<Self, DescriptorError> {
        if capabilities.is_empty() {
            return Err(DescriptorError::NoCapabilities { provider });
        }
        if display_name.trim().is_empty() {
            return Err(DescriptorError::EmptyDisplayName { provider });
        }
        Ok(Self {
            provider,
            display_name,
            capabilities,
            rate,
            cache,
            attribution,
        })
    }

    /// Which provider this describes.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    /// The name to show a user, which is not the id.
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        self.display_name
    }

    /// What it can be asked for.
    #[must_use]
    pub const fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// How often it may be asked.
    #[must_use]
    pub const fn rate(&self) -> &RatePolicy {
        &self.rate
    }

    /// How long its answers may be kept.
    #[must_use]
    pub const fn cache(&self) -> &CachePolicy {
        &self.cache
    }

    /// What must be shown with its data, when its terms require anything.
    #[must_use]
    pub const fn attribution(&self) -> Option<&Attribution> {
        self.attribution.as_ref()
    }

    /// Whether it declares `capability`.
    #[must_use]
    pub const fn can(&self, capability: crate::capability::Capability) -> bool {
        self.capabilities.contains(capability)
    }
}

/// Why a descriptor was refused.
///
/// Every one of these is a programming error at the adapter, which is why they are
/// construction failures and not runtime states: a descriptor is written once, by a person,
/// and a suite runs over it (PG-E5-18) before it can reach a running application.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DescriptorError {
    /// The provider declares no capability.
    #[error("{provider} declares no capability, so it can be asked nothing")]
    NoCapabilities {
        /// The provider with the empty declaration.
        provider: Provider,
    },

    /// The rate window is zero.
    #[error("a rate window has to be longer than zero")]
    EmptyRateWindow,

    /// The cache ceiling is shorter than the freshness.
    #[error("a cache ceiling of {ceiling:?} cannot be shorter than the {max_age:?} freshness")]
    CeilingBelowFreshness {
        /// How long the descriptor says an answer stays fresh.
        max_age: Duration,
        /// The ceiling it also declares.
        ceiling: Duration,
    },

    /// The provider has no display name.
    #[error("{provider} has no display name, so no list can name it")]
    EmptyDisplayName {
        /// The provider with no name.
        provider: Provider,
    },

    /// The attribution notice is empty.
    ///
    /// It does not name the provider, because an `Attribution` is built before the descriptor
    /// it belongs to exists; the adapter that built it is the one place that knows.
    #[error("an attribution notice cannot be empty")]
    EmptyNotice,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).expect("the test passes a non-zero value")
    }

    const DAY: Duration = Duration::from_hours(24);

    fn rate() -> RatePolicy {
        RatePolicy::new(non_zero(40), Duration::from_secs(1), non_zero(10), false)
            .expect("the test's policy is well formed")
    }

    fn cache() -> CachePolicy {
        CachePolicy::new(DAY, Some(180 * DAY), false).expect("the test's policy is well formed")
    }

    fn descriptor(capabilities: Capabilities) -> Result<ProviderDescriptor, DescriptorError> {
        ProviderDescriptor::new(
            Provider::Tmdb,
            "The Movie Database",
            capabilities,
            rate(),
            cache(),
            None,
        )
    }

    #[test]
    fn a_descriptor_keeps_what_it_was_given() {
        let declared = descriptor(Capabilities::all()).expect("the declaration is complete");

        assert_eq!(declared.provider(), Provider::Tmdb);
        assert_eq!(declared.display_name(), "The Movie Database");
        assert_eq!(declared.rate().requests().get(), 40);
        assert_eq!(declared.rate().window(), Duration::from_secs(1));
        assert_eq!(declared.rate().burst().get(), 10);
        assert!(!declared.rate().honours_retry_after());
        assert_eq!(declared.cache().max_age(), DAY);
        assert_eq!(declared.cache().ceiling(), Some(180 * DAY));
        assert!(!declared.cache().revalidates());
        assert!(declared.attribution().is_none());
    }

    #[test]
    fn a_provider_that_can_be_asked_nothing_is_refused() {
        assert_eq!(
            descriptor(Capabilities::EMPTY),
            Err(DescriptorError::NoCapabilities {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn a_provider_with_no_name_is_refused() {
        // The settings list and the diagnostics screen both name it; one blank row is worse
        // than a refusal at construction, which is where a person is still looking.
        let refused = ProviderDescriptor::new(
            Provider::Tmdb,
            "   ",
            Capabilities::all(),
            rate(),
            cache(),
            None,
        );

        assert_eq!(
            refused,
            Err(DescriptorError::EmptyDisplayName {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn a_rate_window_of_zero_is_refused() {
        // A window of zero is not a slow rate, it is a division by zero waiting for the
        // token bucket.
        assert_eq!(
            RatePolicy::new(non_zero(1), Duration::ZERO, non_zero(1), false),
            Err(DescriptorError::EmptyRateWindow)
        );
    }

    #[test]
    fn a_cache_ceiling_shorter_than_the_freshness_is_refused() {
        assert_eq!(
            CachePolicy::new(30 * DAY, Some(DAY), false),
            Err(DescriptorError::CeilingBelowFreshness {
                max_age: 30 * DAY,
                ceiling: DAY
            })
        );
    }

    #[test]
    fn a_ceiling_exactly_at_the_freshness_is_allowed() {
        // The boundary is inclusive: a provider whose term is "keep it a day" and whose data
        // changes daily says the same thing twice, and that is not a contradiction.
        assert!(CachePolicy::new(DAY, Some(DAY), false).is_ok());
    }

    #[test]
    fn a_provider_with_no_ceiling_is_allowed() {
        // AniList and MyAnimeList document no retention term, so "no ceiling" has to be
        // expressible rather than being spelled as a very large number.
        let policy = CachePolicy::new(DAY, None, true).expect("no ceiling is well formed");

        assert!(policy.ceiling().is_none());
        assert!(policy.revalidates());
    }

    #[test]
    fn an_empty_attribution_notice_is_refused() {
        assert_eq!(
            Attribution::new("  ", true),
            Err(DescriptorError::EmptyNotice)
        );
    }

    #[test]
    fn an_attribution_keeps_the_notice_verbatim() {
        // Verbatim is the requirement: TMDB's terms reproduce a fixed sentence, and a
        // trimmed or re-wrapped one is not the sentence they require.
        let notice = "This application uses TMDB and the TMDB APIs but is not endorsed, \
                      certified, or otherwise approved by TMDB.";
        let attribution = Attribution::new(notice, true).expect("the notice is not empty");

        assert_eq!(attribution.notice(), notice);
        assert!(attribution.requires_logo());
    }

    #[test]
    fn a_descriptor_answers_for_its_own_capabilities_and_no_others() {
        use crate::capability::Capability;

        let declared =
            descriptor(Capabilities::EMPTY.with(Capability::Search)).expect("one is enough");

        assert!(declared.can(Capability::Search));
        assert!(!declared.can(Capability::Changes));
    }
}
