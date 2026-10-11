//! Who a read is for.
//!
//! R34 makes the scope a *parameter* of every catalogue, library, episode, progress,
//! watchlist, notification, and search read, and requires content outside it to be
//! indistinguishable from content that does not exist. A parameter is the enforcement that
//! survives a new query being written: a read that forgot the scope does not compile, while
//! ambient state would let one be added quietly.
//!
//! What this module deliberately does not do is decide which ratings a ceiling admits. That
//! mapping is PG-E35-03's, because it needs a table per rating system. Until it exists, a
//! restricted scope fails closed on the two things the schema can already answer: content
//! flagged adult is hidden, and content with no certification is hidden. "Gating fails
//! closed" is the rule, so the absence of the mapping must not mean "show everything".

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The account a read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(i64);

impl AccountId {
    /// Wraps a row id from the `account` table.
    ///
    /// # Errors
    ///
    /// [`AccountError::NotPositive`] for zero and for negative values: `SQLite` numbers
    /// rowids from one, so anything else is not an account that exists, and accepting it
    /// would make "no account" indistinguishable from "account zero".
    pub const fn new(value: i64) -> Result<Self, AccountError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(AccountError::NotPositive { value })
        }
    }

    /// The row id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Why an account identifier was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AccountError {
    /// The value is zero or negative.
    #[error("an account id must be positive, not {value}")]
    NotPositive {
        /// The value that was refused.
        value: i64,
    },
}

/// What an account may see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Everything the catalogue holds.
    Unrestricted,
    /// Everything except adult-flagged content and content with no certification.
    Restricted,
}

/// The account a read serves, and what it may see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountScope {
    account: AccountId,
    visibility: Visibility,
}

impl AccountScope {
    /// A scope that sees everything, for an account with no ceiling and no curation.
    #[must_use]
    pub const fn unrestricted(account: AccountId) -> Self {
        Self {
            account,
            visibility: Visibility::Unrestricted,
        }
    }

    /// A scope that hides adult-flagged and unrated content.
    #[must_use]
    pub const fn restricted(account: AccountId) -> Self {
        Self {
            account,
            visibility: Visibility::Restricted,
        }
    }

    /// The account this scope belongs to.
    #[must_use]
    pub const fn account(self) -> AccountId {
        self.account
    }

    /// What the scope may see.
    #[must_use]
    pub const fn visibility(self) -> Visibility {
        self.visibility
    }

    /// Whether the scope hides anything, which is what a read passes to its query.
    #[must_use]
    pub const fn is_restricted(self) -> bool {
        matches!(self.visibility, Visibility::Restricted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_id_is_positive() {
        assert_eq!(AccountId::new(1).unwrap().get(), 1);
        assert_eq!(AccountId::new(i64::MAX).unwrap().get(), i64::MAX);

        for refused in [0, -1, i64::MIN] {
            let error = AccountId::new(refused).expect_err("a rowid is never zero or negative");
            assert_eq!(error, AccountError::NotPositive { value: refused });
            assert!(error.to_string().contains(&refused.to_string()));
        }
    }

    #[test]
    fn a_scope_carries_the_account_and_its_visibility() {
        let account = AccountId::new(7).unwrap();

        let unrestricted = AccountScope::unrestricted(account);
        assert_eq!(unrestricted.account(), account);
        assert_eq!(unrestricted.visibility(), Visibility::Unrestricted);
        assert!(!unrestricted.is_restricted());

        let restricted = AccountScope::restricted(account);
        assert_eq!(restricted.account(), account);
        assert_eq!(restricted.visibility(), Visibility::Restricted);
        assert!(restricted.is_restricted());
        assert_ne!(
            unrestricted, restricted,
            "the same account may be read in either visibility, and they are not the same request"
        );
    }

    /// The id travels as a bare number, so a stored setting or a DTO does not gain a wrapper
    /// object the moment it crosses a boundary.
    #[test]
    fn an_account_id_serializes_as_its_number() {
        let account = AccountId::new(42).unwrap();
        assert_eq!(serde_json::to_string(&account).unwrap(), "42");
        assert_eq!(serde_json::from_str::<AccountId>("42").unwrap(), account);
    }
}
