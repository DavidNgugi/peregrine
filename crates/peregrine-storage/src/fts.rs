//! The full-text index over `title`: what it holds, how it is maintained, and the read
//! that searches it.
//!
//! `title_fts` is **contentless** (`DATA_MODEL` §2.9): it stores terms and the rowid they
//! came from, and no copy of the text. That is the choice the storage budget asks for — the
//! index is 15–25% of a database that is already the user's library, and storing the corpus
//! twice would spend that again for text that is one join away.
//!
//! It is maintained **explicitly, not by triggers**, and that is not a style preference:
//!
//! * A trigger on `title` could only index `title`'s own columns. Three of the six indexed
//!   columns — aliases, keywords, cast names — are aggregated from `title_alias`,
//!   `title_keyword` and `credit`, so the write that invalidates the row is a write to a
//!   table the trigger is not on, and every one of those tables would need its own trigger
//!   re-running the same aggregate.
//! * A trigger on a contentless table cannot correct an existing row either: `FTS5` has no
//!   way to update a contentless row in place, so an "update" is a delete and a re-insert,
//!   and the delete needs the original text that the contentless table no longer holds.
//! * A trigger fires where the writer does not expect it, which is exactly the kind of hidden
//!   cost the single-writer design exists to make visible.
//!
//! So every maintenance call is a named function, the caller owns the transaction, and the
//! insert path runs it in the same transaction as the row it describes.
//!
//! The cost of that choice is stated plainly, because it is the thing that can go wrong:
//! **a write to `title_alias`, `title_keyword`, `credit` or `person` does not update the
//! index.** Nothing watches those tables, so the caller that changes them has to call
//! [`reindex`], which is what the provider sync must do when it fills a title's aliases,
//! keywords, or cast. Until it does, the index describes the title as it was. `rebuild`
//! re-reads every source table and is what reconciles an index that has drifted.
//!
//! One projection — [`index_projection!`] — defines what the index holds, and both the
//! per-title reindex and the whole-index rebuild are built from it, so the two cannot come
//! to describe different rows.
//!
//! The read takes an [`AccountScope`] and applies it in SQL (AGENTS R34): a restricted
//! account gets the same answer for a hidden title as for one that does not exist.
//!
//! Relevance ranking is deliberately absent. Ordering is by rowid, which is stable under
//! insertion; `bm25()` is relative to the whole corpus, so a cursor taken from it does not
//! survive a row being inserted between two pages. Prefix-term weighting, `bm25` weights, and
//! the popularity tiebreak belong to the query builder in PG-E8-01, and the ranking plan to
//! `SPIKE-5`.

use peregrine_domain::AccountScope;
use sqlx::Connection as _;
use sqlx::{SqliteConnection, SqlitePool};

use crate::catalogue::{IdentityRow, TitleIdentity, hidden_by_scope};
use crate::error::StorageError;
use crate::page::PageRequest;

/// The projection that defines one searchable title: the single statement of what the index
/// holds.
///
/// It is a macro rather than a constant so both statements below can be built from it with
/// `concat!`. `sqlx` 0.9 accepts only a `&'static str` as a statement and a `const` cannot
/// be spliced into one, so this is the way to have one definition rather than two that can
/// drift apart.
///
/// The index row's rowid is the title's id, which is what makes the join possible on a
/// contentless table: there are no column values to match, so the rowid is the whole link.
///
/// Every aggregate is ordered inside the call. `group_concat` promises nothing about the
/// order of the rows it concatenates, and the token order inside a column is what a phrase
/// query matches against — so an unordered concatenation would make the same source rows
/// produce different search results on different runs, which is precisely what the rebuild
/// guarantee is about.
macro_rules! index_projection {
    () => {
        "SELECT t.id, t.name, t.original_name, \
         (SELECT group_concat(a.alias, ' ' ORDER BY a.alias, a.region) \
            FROM title_alias a WHERE a.title_id = t.id), \
         t.overview, \
         (SELECT group_concat(k.name, ' ' ORDER BY k.name) \
            FROM title_keyword tk JOIN keyword k ON k.id = tk.keyword_id \
           WHERE tk.title_id = t.id), \
         (SELECT group_concat(p.name, ' ' ORDER BY c.order_index, p.name) \
            FROM credit c JOIN person p ON p.id = c.person_id \
           WHERE c.title_id = t.id AND c.kind = 'cast') \
         FROM title t"
    };
}

/// Writes one title's index row, on the caller's connection and in the caller's transaction.
///
/// # Errors
///
/// The driver's error: the statement can only fail for a reason the caller cannot act on.
pub(crate) async fn index(
    connection: &mut SqliteConnection,
    title_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(concat!(
        "INSERT INTO title_fts (rowid, name, original_name, aliases, overview, keywords, cast_names) ",
        index_projection!(),
        " WHERE t.id = ?1"
    ))
    .bind(title_id)
    .execute(connection)
    .await?;
    // No row means the projection found no `title` row — a caller reindexing an id that was
    // deleted underneath it. Reporting it is what lets `Database::reindex_title` say so
    // rather than claim it indexed something.
    Ok(result.rows_affected() > 0)
}

/// Removes one title's index row, on the caller's connection and in the caller's transaction.
///
/// A plain `DELETE` needs `contentless_delete=1` on the table; without it `SQLite` refuses
/// the statement, and with `content=''` alone the alternative is `delete-all` and a full
/// rebuild per removed title.
///
/// # Errors
///
/// The driver's error.
pub(crate) async fn unindex(
    connection: &mut SqliteConnection,
    title_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM title_fts WHERE rowid = ?1")
        .bind(title_id)
        .execute(connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Rewrites one title's index row: delete then insert, because a contentless table has no
/// update in place.
///
/// This is the "update" the work package asks for, and it is also what the insert path
/// calls: rewriting is idempotent, so a title inserted twice cannot end up indexed twice.
///
/// # Errors
///
/// The driver's error.
pub(crate) async fn reindex(
    connection: &mut SqliteConnection,
    title_id: i64,
) -> Result<bool, sqlx::Error> {
    unindex(connection, title_id).await?;
    index(connection, title_id).await
}

/// Rebuilds the whole index from the source tables, reporting how many titles it indexed.
///
/// `delete-all` empties the index and the projection then re-reads every title, so the
/// rebuild is defined by the source tables alone: it cannot inherit a wrong row from the
/// index it replaces. It is the recovery path after a dataset reimport or a change to what
/// the projection reads (`DATA_MODEL` §6), not a step the ordinary write path needs.
///
/// It runs on the caller's connection — the writer's, never a second one, because a rebuild
/// that raced the write path would interleave its `delete-all` with a title being indexed.
///
/// # Errors
///
/// The driver's error.
pub(crate) async fn rebuild(connection: &mut SqliteConnection) -> Result<u64, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    sqlx::query("INSERT INTO title_fts (title_fts) VALUES ('delete-all')")
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query(concat!(
        "INSERT INTO title_fts (rowid, name, original_name, aliases, overview, keywords, cast_names) ",
        index_projection!()
    ))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(result.rows_affected())
}

/// Merges the index's b-tree segments, which is `FTS5`'s `optimize`.
///
/// An `FTS5` table is written as a series of immutable segments, one per transaction that
/// touched it, and a term is looked up in each of them; the index is only merged when it is
/// asked to be. That is what `optimize` is for, and it is why the maintenance job runs it
/// (`DATA_MODEL` §6): left alone, an index written a title at a time degrades into as many
/// segments as there were writes. It is idempotent — a second call on a merged index has
/// nothing to merge — and it is defined by the index's own contents, so it cannot change what
/// a search matches.
///
/// It runs on the caller's connection, which is the writer's: this is a write to the index,
/// and the single-writer rule (`ARCHITECTURE` §5) is what keeps it from interleaving with a
/// title being indexed.
///
/// # Errors
///
/// The driver's error.
pub(crate) async fn optimize(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO title_fts (title_fts) VALUES ('optimize')")
        .execute(connection)
        .await?;
    Ok(())
}

/// The `MATCH` expression for a user's query, or `None` when there is nothing to match.
///
/// `FTS5` has its own query grammar, and a user string is not a query: `"` alone is
/// `unterminated string`, `*` alone is `unknown special query`, and `AND`, `-`, and `^`
/// alone are all syntax errors against the bundled SQLite. Reaching that grammar from
/// untrusted input is not an option, so no character of the input is ever parsed as grammar:
/// each whitespace-separated run becomes one double-quoted `FTS5` string — where the only
/// escape, `""`, is the quote itself — and the terms are `AND`ed.
///
/// Each term is used as a prefix, so a partially typed word matches, which is what a search
/// box that runs on every keystroke needs.
///
/// `None` rather than an empty expression: `MATCH ''` is itself a syntax error, so an empty
/// or whitespace-only query is answered without reaching the database at all. A `NUL` byte
/// is dropped for the same reason — `SQLite` reports `unterminated string` for it inside a
/// quoted string, and it is not a term anyone can see they typed.
#[must_use]
pub(crate) fn match_expression(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for term in query.split_whitespace() {
        let term = term.replace('\0', "");
        if term.is_empty() {
            continue;
        }
        terms.push(format!("\"{}\"*", term.replace('"', "\"\"")));
    }
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

/// One page of titles `scope` may see that match `query`, the highest id first.
///
/// The scope and the cursor are branches of the `WHERE` clause, so a restricted page is
/// still a page: hidden rows do not consume the limit, and a caller cannot tell a row it may
/// not see from a row that is not there. The join back to `title` is an inner one, so an
/// index row whose title has gone is not a result either.
///
/// Ordering is by `f.rowid`, which the planner serves from the index's own rowid order:
/// `ORDER BY t.id` needs a temporary b-tree over every match, while `ORDER BY f.rowid` walks
/// the index in order and stops at the limit. It is also what makes the cursor stable — see
/// the module note on ranking.
///
/// # Errors
///
/// [`StorageError::Statement`] when the statement fails. A query the user typed cannot be
/// the cause: [`match_expression`] is what turns it into one the grammar accepts.
pub(crate) async fn search(
    pool: &SqlitePool,
    scope: AccountScope,
    query: &str,
    request: PageRequest,
) -> Result<Vec<TitleIdentity>, StorageError> {
    search_kind(pool, scope, query, request, None).await
}

pub(crate) async fn search_kind(
    pool: &SqlitePool,
    scope: AccountScope,
    query: &str,
    request: PageRequest,
    kind: Option<crate::catalogue::TitleKind>,
) -> Result<Vec<TitleIdentity>, StorageError> {
    let Some(expression) = match_expression(query) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query_as::<_, IdentityRow>(
        "SELECT t.id, t.kind, t.subtype, t.name, t.sort_name, t.metadata_state, t.adult \
         FROM title_fts f JOIN title t ON t.id = f.rowid \
         WHERE title_fts MATCH ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR f.rowid < ?3) \
           AND (?5 IS NULL OR t.kind = ?5) \
         ORDER BY f.rowid DESC LIMIT ?4",
    )
    .bind(expression)
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .bind(kind.map(crate::catalogue::TitleKind::as_str))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    if rows.is_empty() {
        // **The index has no term for what the reader typed, and that is not the same as no
        // match.** "Spider-Man" is `spider` + `man` to the tokenizer, so a reader who types
        // "spiderman" has nothing to match; the folded read is the one that closes that gap.
        return search_folded(pool, scope, query, request, kind).await;
    }
    rows.into_iter().map(TryInto::try_into).collect()
}

/// The read an index miss falls back to: a folded name match over the scoped catalogue.
///
/// **The index tokenises by word, and a reader does not.** "Spider-Man" is `spider` + `man` to
/// `unicode61`, so a reader who types "spiderman" has nothing to match. This folds the punctuation
/// out of both sides — the title's name and original name against the query — and compares the
/// result. It is deliberately a scan, because it only runs for a query the index found nothing for,
/// and the catalogue's size makes that cheap. The trigram index and the ranking weights that would
/// replace it are `PG-E8-04` and `PG-E8-01`.
///
/// **The join back to the index is the index's contract.** A title whose index row is gone is
/// unsearchable, and that has to keep being true; a fallback that read `title` directly would make an
/// unindexed title findable again and hide a broken index from the rebuild test that exists to catch
/// one.
///
/// `sqlx` 0.9 only accepts a `&'static str` here, which is why the expression is written out rather
/// than built by a helper: at runtime it would be the same statement with a leaked string.
const FOLDED_SEARCH: &str = r"
SELECT t.id, t.kind, t.subtype, t.name, t.sort_name, t.metadata_state, t.adult
FROM title t
JOIN title_fts f ON f.rowid = t.id
WHERE (
  lower(replace(replace(replace(replace(replace(t.name, '-', ''), ' ', ''), '''', ''), ':', ''), '.', ''))
    LIKE '%' || ?1 || '%'
  OR lower(replace(replace(replace(replace(replace(t.original_name, '-', ''), ' ', ''), '''', ''), ':', ''), '.', ''))
    LIKE '%' || ?1 || '%'
)
  AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))
  AND (?3 IS NULL OR t.id < ?3)
  AND (?5 IS NULL OR t.kind = ?5)
ORDER BY t.id DESC LIMIT ?4
";

/// The same folding as `FOLDED_SEARCH`, applied to the reader's own text.
///
/// ASCII lowercase rather than Unicode, because that is what SQLite's `lower()` does to the column;
/// accents and non-ASCII case are the index's job. The cap is the guard the FTS sanitizer gives a
/// term, for the same reason: a 100 000-character "term" is not a search, and a LIKE pattern that
/// long is refused by SQLite before it can answer.
fn folded_query(query: &str) -> String {
    query
        .chars()
        .filter(|character| {
            !character.is_whitespace() && !matches!(character, '-' | '\'' | ':' | '.')
        })
        .collect::<String>()
        .to_ascii_lowercase()
}

const MAX_FOLDED_QUERY_CHARS: usize = 256;

async fn search_folded(
    pool: &SqlitePool,
    scope: AccountScope,
    query: &str,
    request: PageRequest,
    kind: Option<crate::catalogue::TitleKind>,
) -> Result<Vec<TitleIdentity>, StorageError> {
    let needle = folded_query(query);
    if needle.is_empty() || needle.chars().count() > MAX_FOLDED_QUERY_CHARS {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, IdentityRow>(FOLDED_SEARCH)
        .bind(needle)
        .bind(hidden_by_scope(scope))
        .bind(request.bindable_cursor())
        .bind(request.bindable_limit())
        .bind(kind.map(crate::catalogue::TitleKind::as_str))
        .fetch_all(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::{StubTitle, TitleKind, TitleSubtype, insert_stub};
    use crate::database::{Database, open_reader_pool, open_writer_connection};
    use crate::migrations;
    use crate::page::Cursor;
    use peregrine_domain::AccountId;
    use tempfile::TempDir;

    /// Injected, never the wall clock (AGENTS R14).
    const NOW: i64 = 1_700_000_000_000;

    fn stub(name: &str) -> StubTitle {
        StubTitle {
            kind: TitleKind::Movie,
            subtype: TitleSubtype::Standard,
            name: name.to_owned(),
            original_name: None,
            sort_name: name.to_lowercase(),
            at: NOW,
        }
    }

    /// A migrated writer connection and the reader pool a search reads through.
    ///
    /// The writer connection is the one the product's writer task would own, so a test can
    /// call the maintenance functions at the layer that owns the transaction.
    async fn fixture(directory: &TempDir) -> (SqliteConnection, SqlitePool) {
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();
        let readers = open_reader_pool(&path).await.unwrap();
        (connection, readers)
    }

    /// Inserts a title the way the write path does — which indexes it — and then sets the two
    /// columns a scope reads, which the index does not hold and so does not care about.
    async fn rated(
        connection: &mut SqliteConnection,
        name: &str,
        adult: i64,
        rating: Option<&str>,
    ) -> i64 {
        let id = insert_stub(connection, &stub(name)).await.unwrap();
        sqlx::query("UPDATE title SET adult = ?1, content_rating = ?2 WHERE id = ?3")
            .bind(adult)
            .bind(rating)
            .bind(id)
            .execute(&mut *connection)
            .await
            .unwrap();
        id
    }

    fn any_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
    }

    fn restricted() -> AccountScope {
        AccountScope::restricted(AccountId::new(2).expect("account two exists"))
    }

    fn ids_of(titles: &[TitleIdentity]) -> Vec<i64> {
        titles.iter().map(|title| title.id).collect()
    }

    /// How many rows the index holds, which is how a test asks whether maintenance ran.
    ///
    /// A test helper rather than a product read: nothing in the application asks for the
    /// count, and the maintenance job that will want it (PG-E3-16) can add the read then.
    async fn index_size(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM title_fts")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The ids one page of results holds, in the order it was read.
    async fn hits(pool: &SqlitePool, scope: AccountScope, query: &str) -> Vec<i64> {
        ids_of(
            &search(pool, scope, query, PageRequest::first(50))
                .await
                .unwrap(),
        )
    }

    /// A request built from the id a page ended on, which is what a caller does with a cursor.
    fn below(id: i64, limit: u32) -> PageRequest {
        PageRequest::after(Cursor::new(id), limit)
    }

    /// Step zero, kept: the whole index rests on `FTS5` being compiled into the bundled
    /// `SQLite`. A build that lost it would fail here with the reason, rather than as an
    /// obscure `no such module: fts5` inside every other test in this module.
    #[tokio::test]
    async fn the_bundled_sqlite_compiles_fts5_in() {
        let directory = TempDir::new().unwrap();
        let (mut connection, _) = fixture(&directory).await;

        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        let compiled_in: i64 =
            sqlx::query_scalar("SELECT sqlite_compileoption_used('ENABLE_FTS5')")
                .fetch_one(&mut connection)
                .await
                .unwrap();

        assert_eq!(
            compiled_in, 1,
            "the search index is an FTS5 table and the bundled SQLite is {version}, \
             which does not report ENABLE_FTS5"
        );
    }

    /// The sanitizer is the whole defence against `FTS5`'s query grammar, so it is asserted
    /// on its own rather than only through a reachable outcome.
    #[test]
    fn a_match_expression_never_contains_anything_but_quoted_terms() {
        assert_eq!(match_expression("Alien"), Some("\"Alien\"*".to_owned()));
        assert_eq!(
            match_expression("  the   matrix "),
            Some("\"the\"* AND \"matrix\"*".to_owned()),
            "whitespace separates terms and is otherwise not part of them"
        );
        assert_eq!(
            match_expression("\"\""),
            Some("\"\"\"\"\"\"*".to_owned()),
            "a quote is escaped by doubling it, which is FTS5's only escape"
        );
        assert_eq!(
            match_expression("NEAR(a b)"),
            Some("\"NEAR(a\"* AND \"b)\"*".to_owned()),
            "NEAR is a word here, not an operator"
        );
        assert_eq!(match_expression("AND"), Some("\"AND\"*".to_owned()));
        assert_eq!(match_expression("*"), Some("\"*\"*".to_owned()));

        assert_eq!(match_expression(""), None, "an empty query is not a query");
        assert_eq!(match_expression("   \t\n "), None);
        assert_eq!(
            match_expression("\0"),
            None,
            "a NUL byte is not a term, and FTS5 rejects it inside a quoted string"
        );
        assert_eq!(
            match_expression("a\0b"),
            Some("\"ab\"*".to_owned()),
            "a NUL byte inside a term is dropped rather than left to the driver"
        );
    }

    /// Hostile input, end to end: every one of these is a syntax error, a panic, or a wrong
    /// answer if the string ever reaches `MATCH` unquoted.
    #[tokio::test]
    async fn hostile_query_strings_are_answered_rather_than_refused() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;
        let alien = rated(&mut connection, "Alien", 0, Some("R")).await;

        let long_token = "a".repeat(100_000);
        let many_terms = "alien ".repeat(5_000);
        let hostile: Vec<&str> = vec![
            "\"",
            "*",
            "NEAR(a b)",
            "AND",
            "-",
            "^",
            "'",
            "a AND",
            "AND AND",
            "NEAR(",
            "\"unterminated",
            "col:x",
            "a:b",
            "{}",
            "...",
            "---",
            "NOT",
            "\0",
            "\0\0",
            "\n\t ",
            "\"\"\"\"",
            "😀",
            &long_token,
            &many_terms,
        ];

        for query in hostile {
            let found = search(&readers, any_account(), query, PageRequest::first(20))
                .await
                .unwrap_or_else(|error| panic!("{query:?} must be a query, not an error: {error}"));
            assert!(
                found.iter().all(|title| title.id == alien),
                "{query:?} returned a row that does not match: {found:?}"
            );
        }

        // `long_token` is one term of a hundred thousand characters: it matches nothing, but
        // the statement still has to run. `many_terms` is 5 000 terms ANDed; it matches the
        // fixture, so the sanitizer cannot have silently dropped the query.
        assert_eq!(
            hits(&readers, any_account(), &long_token).await,
            Vec::<i64>::new()
        );
        assert_eq!(
            hits(&readers, any_account(), &many_terms).await,
            vec![alien]
        );
    }

    /// The empty cases, together: no index and no results are the same answer, not an error.
    #[tokio::test]
    async fn an_empty_index_and_a_query_that_matches_nothing_both_return_nothing() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;

        assert_eq!(index_size(&readers).await, 0);
        assert_eq!(
            hits(&readers, any_account(), "Alien").await,
            Vec::<i64>::new()
        );
        assert_eq!(
            search(&readers, any_account(), "Alien", PageRequest::first(10))
                .await
                .unwrap(),
            Vec::<TitleIdentity>::new(),
            "an empty index is an empty page"
        );

        let alien = rated(&mut connection, "Alien", 0, Some("R")).await;
        assert_eq!(hits(&readers, any_account(), "Alien").await, vec![alien]);
        assert_eq!(
            hits(&readers, any_account(), "Zzzz").await,
            Vec::<i64>::new()
        );
        assert_eq!(hits(&readers, any_account(), "").await, Vec::<i64>::new());
    }

    /// The insert path maintains the index in the same transaction as the row, so a title
    /// that commits is searchable and no rebuild is needed to find it.
    #[tokio::test]
    async fn a_title_inserted_through_the_writer_is_searchable_without_a_rebuild() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let database = Database::open(&path).await.unwrap();

        let alien = database
            .insert_stub_title(stub("Alien"))
            .await
            .expect("the writer inserts and indexes");

        let found = database
            .search(any_account(), "Alien", PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(
            ids_of(&found),
            vec![alien],
            "the write path indexes what it writes"
        );

        // The row came back through the reader pool, which the writer never shares a
        // connection with, so this is the committed state rather than a transaction in flight.
        let readers = open_reader_pool(&path).await.unwrap();
        assert_eq!(index_size(&readers).await, 1);
    }

    /// Insert, update, and delete of one index row, which is what "explicit maintenance"
    /// means: each is a call, and the row is available to the repair path.
    #[tokio::test]
    async fn reindex_and_unindex_follow_the_source_row() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;
        let alien = rated(&mut connection, "Alien", 0, Some("R")).await;

        assert_eq!(hits(&readers, any_account(), "Alien").await, vec![alien]);

        // A provider sync renames the title; the index is only right once it is told.
        sqlx::query("UPDATE title SET name = 'Xenomorph' WHERE id = ?1")
            .bind(alien)
            .execute(&mut connection)
            .await
            .unwrap();
        assert!(reindex(&mut connection, alien).await.unwrap());
        assert_eq!(
            hits(&readers, any_account(), "Xenomorph").await,
            vec![alien]
        );
        assert!(
            hits(&readers, any_account(), "Alien").await.is_empty(),
            "the rewrite removes the term the source no longer holds"
        );
        assert_eq!(
            index_size(&readers).await,
            1,
            "an update is a rewrite, not a second row"
        );

        // Running it twice is idempotent: the ordinary case is a sync that notices no change.
        assert!(reindex(&mut connection, alien).await.unwrap());
        assert_eq!(index_size(&readers).await, 1);

        // A deletion of the index row alone, which is what a title removal does.
        assert!(unindex(&mut connection, alien).await.unwrap());
        assert_eq!(
            hits(&readers, any_account(), "Xenomorph").await,
            Vec::<i64>::new()
        );
        assert_eq!(index_size(&readers).await, 0);
        assert!(
            !unindex(&mut connection, alien).await.unwrap(),
            "removing a row that is not there reports that it removed nothing"
        );

        // The title row is untouched: this removes the index row, not the title.
        let titles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM title")
            .fetch_one(&readers)
            .await
            .unwrap();
        assert_eq!(titles, 1);
    }

    /// The three aggregated columns are the reason the index cannot be a trigger on `title`:
    /// their text lives in other tables, and each is searchable once it is aggregated in.
    #[tokio::test]
    async fn aliases_keywords_and_cast_names_are_part_of_the_indexed_text() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;
        let alien = rated(&mut connection, "Alien", 0, Some("R")).await;

        for statement in [
            "INSERT INTO title_alias (title_id, alias, region, kind) VALUES (?1, 'Starbeast', 'US', 'alternative')",
            "INSERT INTO keyword (name) VALUES ('space horror')",
            "INSERT INTO title_keyword (title_id, keyword_id) SELECT ?1, id FROM keyword WHERE name = 'space horror'",
            "INSERT INTO person (name, sort_name) VALUES ('Sigourney Weaver', 'weaver, sigourney')",
            "INSERT INTO credit (title_id, person_id, kind, order_index) SELECT ?1, id, 'cast', 0 FROM person WHERE name = 'Sigourney Weaver'",
        ] {
            sqlx::query(statement)
                .bind(alien)
                .execute(&mut connection)
                .await
                .unwrap_or_else(|error| panic!("{statement}: {error}"));
        }

        assert!(reindex(&mut connection, alien).await.unwrap());

        assert_eq!(
            hits(&readers, any_account(), "Starbeast").await,
            vec![alien],
            "an alias is searchable"
        );
        assert_eq!(
            hits(&readers, any_account(), "horror").await,
            vec![alien],
            "a keyword is searchable"
        );
        assert_eq!(
            hits(&readers, any_account(), "Sigourney").await,
            vec![alien],
            "a cast name is searchable"
        );

        // The column values themselves are not in the index: it is contentless, and the
        // only thing it can answer with is the rowid the read then joins on.
        let stored: Option<String> =
            sqlx::query_scalar("SELECT name FROM title_fts WHERE rowid = ?1")
                .bind(alien)
                .fetch_one(&readers)
                .await
                .unwrap();
        assert_eq!(
            stored, None,
            "a contentless table holds no copy of the text it indexes"
        );
    }

    /// The acceptance: a rebuild reproduces identical search results.
    ///
    /// Identical means the same ids in the same order, for every query, including after the
    /// index has been made wrong in both directions — a row that should not be there and a
    /// row that should. Membership alone would not catch a rebuild that reordered a page.
    #[tokio::test]
    async fn a_rebuild_reproduces_identical_search_results() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;

        let alien = rated(&mut connection, "Alien", 0, Some("R")).await;
        let aliens = rated(&mut connection, "Aliens", 0, Some("R")).await;
        let blade = rated(&mut connection, "Blade Runner", 0, Some("R")).await;
        let dune = rated(&mut connection, "Dune", 0, Some("PG-13")).await;
        let arr = rated(&mut connection, "Alien Resurrection", 0, Some("R")).await;
        sqlx::query("INSERT INTO title_alias (title_id, alias, region, kind) VALUES (?1, 'Starbeast', NULL, 'alternative')")
            .bind(alien)
            .execute(&mut connection)
            .await
            .unwrap();

        // The baseline is the index the source tables describe, so the fixture that wrote the
        // alias has to say so: nothing indexes `title_alias`, and a baseline taken from the
        // stale index would be a baseline the rebuild is right to differ from. That delay is
        // the cost of explicit maintenance, and `reindex` is what closes it.
        assert_eq!(rebuild(&mut connection).await.unwrap(), 5);

        // Every query whose result the rebuild has to reproduce.
        let queries = [
            "Alien",
            "ali",
            "Blade",
            "dune",
            "Starbeast",
            "Runner",
            "zzzz",
        ];

        let mut before: Vec<(String, Vec<i64>)> = Vec::new();
        for query in queries {
            before.push((query.to_owned(), hits(&readers, any_account(), query).await));
        }
        assert_eq!(
            before[0].1,
            vec![arr, aliens, alien],
            "the fixture must match, newest first, before anything is broken"
        );

        // Break the index in both directions: an entry with no title behind it, and a title
        // with no entry.
        sqlx::query("INSERT INTO title_fts (rowid, name) VALUES (9001, 'Zzzzghost')")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("DELETE FROM title_fts WHERE rowid = ?1")
            .bind(blade)
            .execute(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            index_size(&readers).await,
            5,
            "six entries for five titles: one ghost added, one title lost"
        );

        let mut broken = Vec::new();
        for query in queries {
            broken.push((query.to_owned(), hits(&readers, any_account(), query).await));
        }
        assert_ne!(
            before, broken,
            "the broken index must actually change a result, or this test proves nothing"
        );
        assert_eq!(
            hits(&readers, any_account(), "Zzzzghost").await,
            Vec::<i64>::new(),
            "an index row with no title behind it is not a result"
        );

        let indexed = rebuild(&mut connection).await.unwrap();
        assert_eq!(indexed, 5, "one index row per title, and no more");
        assert_eq!(index_size(&readers).await, 5);

        let mut after: Vec<(String, Vec<i64>)> = Vec::new();
        for query in queries {
            after.push((query.to_owned(), hits(&readers, any_account(), query).await));
        }
        assert_eq!(
            before, after,
            "a rebuild must reproduce the same ids in the same order"
        );

        // Rebuilding an index that is already correct changes nothing: the rebuild is not
        // itself a source of drift.
        assert_eq!(rebuild(&mut connection).await.unwrap(), 5);
        let mut again = Vec::new();
        for query in queries {
            again.push((query.to_owned(), hits(&readers, any_account(), query).await));
        }
        assert_eq!(before, again);

        // The rebuilt index still holds what the source holds, including the aggregate.
        assert_eq!(
            hits(&readers, any_account(), "Starbeast").await,
            vec![alien]
        );
        assert_eq!(
            hits(&readers, any_account(), "Blade").await,
            vec![blade],
            "the title the broken index had lost is back"
        );
        assert_eq!(
            hits(&readers, any_account(), "Dune").await,
            vec![dune],
            "a title that was never broken reads the same as before"
        );
    }

    /// A search page is a page: hidden rows must not consume the limit, and the scope must
    /// be applied before the limit rather than after it.
    #[tokio::test]
    async fn a_restricted_scope_gets_the_same_answer_for_hidden_and_absent_content() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;

        let rated_title = rated(&mut connection, "Alien Rated", 0, Some("R")).await;
        let unrated = rated(&mut connection, "Alien Unrated", 0, None).await;
        let adult = rated(&mut connection, "Alien Adult", 1, Some("X")).await;

        // The unrestricted account sees all three.
        assert_eq!(
            hits(&readers, any_account(), "Alien").await,
            vec![adult, unrated, rated_title],
            "the unrestricted scope sees every match, newest first"
        );

        // The restricted account sees the rated one and cannot tell the other two from ids
        // that do not exist.
        assert_eq!(
            hits(&readers, restricted(), "Alien").await,
            vec![rated_title]
        );

        // A title the scope hides and a term that matches nothing are the same empty page,
        // and a cursor taken at a hidden row reads what a cursor past the end reads.
        assert_eq!(
            hits(&readers, restricted(), "Unrated").await,
            hits(&readers, restricted(), "nosuchterm").await,
            "hidden content answers like absent content"
        );
        for hidden in [unrated, adult] {
            assert_eq!(
                search(&readers, restricted(), "Alien", below(hidden, 10))
                    .await
                    .unwrap(),
                search(&readers, restricted(), "Alien", below(i64::MAX, 10))
                    .await
                    .unwrap(),
                "a hidden title and an id past the catalogue are the same cursor"
            );
        }

        // A query that matches only hidden content is an empty page, not an error.
        assert_eq!(
            hits(&readers, restricted(), "Adult").await,
            Vec::<i64>::new()
        );

        // The rows are there; the scope is what hides them.
        assert_eq!(hits(&readers, any_account(), "Adult").await, vec![adult]);
        assert_eq!(
            hits(&readers, any_account(), "Unrated").await,
            vec![unrated]
        );
    }

    /// A restricted page holds as many visible rows as the limit allows, even when every
    /// visible row is separated by hidden ones. A filter applied after the query would
    /// return one row for a limit of two and eventually none at all.
    #[tokio::test]
    async fn a_restricted_page_is_still_a_full_page() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;

        let mut visible = Vec::new();
        for index in 0..4 {
            visible.push(rated(&mut connection, &format!("Alien {index}"), 0, Some("R")).await);
            let _ = rated(&mut connection, &format!("Alien hidden {index}"), 0, None).await;
        }

        let first = search(&readers, restricted(), "Alien", PageRequest::first(2))
            .await
            .unwrap();
        assert_eq!(
            ids_of(&first).len(),
            2,
            "two visible rows, not two minus one"
        );
        assert_eq!(ids_of(&first), vec![visible[3], visible[2]]);

        let second = search(
            &readers,
            restricted(),
            "Alien",
            below(first.last().unwrap().id, 2),
        )
        .await
        .unwrap();
        assert_eq!(ids_of(&second), vec![visible[1], visible[0]]);

        let third = search(
            &readers,
            restricted(),
            "Alien",
            below(second.last().unwrap().id, 2),
        )
        .await
        .unwrap();
        assert!(third.is_empty(), "the pages end where the visible rows do");
    }

    /// The cursor is the id a page ended on, so a row inserted above it cannot push a row
    /// out of the next page: nothing is skipped and nothing is read twice.
    #[tokio::test]
    async fn a_search_page_neither_skips_nor_repeats_when_rows_are_inserted_between_pages() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;

        let mut original = Vec::new();
        for index in 0..5 {
            original.push(rated(&mut connection, &format!("Alien {index}"), 0, Some("R")).await);
        }

        let mut seen = Vec::new();
        let first = search(&readers, any_account(), "Alien", PageRequest::first(2))
            .await
            .unwrap();
        seen.extend(ids_of(&first));
        let mut cursor = first.last().unwrap().id;

        // Three new matching titles arrive between the pages. They sort above the cursor, so
        // they belong to a page the caller has already read.
        for index in 5..8 {
            let inserted = rated(&mut connection, &format!("Alien {index}"), 0, Some("R")).await;
            assert!(inserted > cursor, "a new row is above the cursor");
        }

        loop {
            let page = search(&readers, any_account(), "Alien", below(cursor, 2))
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            cursor = page.last().unwrap().id;
            seen.extend(ids_of(&page));
        }

        let mut expected = original.clone();
        expected.reverse();
        assert_eq!(
            seen, expected,
            "each original title exactly once, in the order the ordering promises"
        );
        assert_eq!(index_size(&readers).await, 8, "all eight are indexed");
        assert_eq!(
            hits(&readers, any_account(), "Alien").await.len(),
            8,
            "and a first page read now does see the new rows"
        );
    }

    /// Locale edge case: diacritics are folded, and a combining accent is the same word as
    /// the precomposed one.
    #[tokio::test]
    async fn unicode_and_combining_characters_in_a_title_are_searchable() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;

        let amelie = rated(&mut connection, "Amélie", 0, Some("R")).await;
        // `e` followed by U+0301 COMBINING ACUTE ACCENT: the same word, written differently.
        let noir = rated(&mut connection, "Cafe\u{0301} Noir", 0, Some("R")).await;
        let japanese = rated(&mut connection, "君の名は。", 0, Some("R")).await;

        assert_eq!(hits(&readers, any_account(), "Amélie").await, vec![amelie]);
        assert_eq!(
            hits(&readers, any_account(), "amelie").await,
            vec![amelie],
            "`remove_diacritics 2` folds the accent"
        );
        assert_eq!(hits(&readers, any_account(), "café").await, vec![noir]);
        assert_eq!(
            hits(&readers, any_account(), "cafe").await,
            vec![noir],
            "a combining accent folds to the same token as a precomposed one"
        );
        assert_eq!(
            hits(&readers, any_account(), "君の名は").await,
            vec![japanese]
        );
    }

    /// A compound the tokenizer splits is still found by the spelling a person types.
    #[tokio::test]
    async fn a_compound_the_index_splits_is_found_by_its_folded_name() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;
        let spider = rated(
            &mut connection,
            "Spider-Man: Brand New Day",
            0,
            Some("PG-13"),
        )
        .await;
        let speed = rated(&mut connection, "Speed", 0, Some("R")).await;

        assert_eq!(
            hits(&readers, any_account(), "spiderman").await,
            vec![spider],
            "the hyphen and the spaces are folded out of both sides"
        );
        assert_eq!(
            hits(&readers, any_account(), "spider man").await,
            vec![spider],
            "spacing is a spelling, not a term"
        );
        assert_eq!(
            hits(&readers, any_account(), "speed").await,
            vec![speed],
            "the index answers first and the fallback does not take its place"
        );
    }

    /// An index row whose title is gone is not a result. There is no foreign key from a
    /// virtual table to `title`, so this is the join's job, and it is worth a test because
    /// the index can hold such a row after a delete.
    #[tokio::test]
    async fn an_index_row_with_no_title_behind_it_is_not_a_result() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;
        let alien = rated(&mut connection, "Alien", 0, Some("R")).await;

        assert_eq!(hits(&readers, any_account(), "Alien").await, vec![alien]);

        sqlx::query("DELETE FROM title WHERE id = ?1")
            .bind(alien)
            .execute(&mut connection)
            .await
            .unwrap();

        assert_eq!(
            index_size(&readers).await,
            1,
            "the index row is still there: nothing told the index about the delete"
        );
        assert_eq!(
            hits(&readers, any_account(), "Alien").await,
            Vec::<i64>::new(),
            "a row with no title is absent from a search, not a dangling result"
        );

        // A rebuild is what reconciles it.
        assert_eq!(rebuild(&mut connection).await.unwrap(), 0);
        assert_eq!(index_size(&readers).await, 0);
    }
    #[tokio::test]
    async fn title_kind_filter_precedes_limit_and_preserves_account_scope() {
        let directory = TempDir::new().unwrap();
        let (mut connection, readers) = fixture(&directory).await;
        let movie = rated(&mut connection, "Alien movie", 0, Some("PG")).await;
        let show = rated(&mut connection, "Alien series", 0, Some("PG")).await;
        sqlx::query("UPDATE title SET kind = 'show' WHERE id = ?1")
            .bind(show)
            .execute(&mut connection)
            .await
            .unwrap();
        let hidden = rated(&mut connection, "Alien hidden", 1, Some("PG")).await;
        assert_eq!(
            ids_of(
                &search_kind(
                    &readers,
                    restricted(),
                    "Alien",
                    PageRequest::first(1),
                    Some(TitleKind::Movie)
                )
                .await
                .unwrap()
            ),
            vec![movie]
        );
        assert_eq!(
            ids_of(
                &search_kind(
                    &readers,
                    any_account(),
                    "Alien",
                    PageRequest::first(1),
                    Some(TitleKind::Movie)
                )
                .await
                .unwrap()
            ),
            vec![hidden]
        );
        assert_eq!(
            ids_of(
                &search_kind(
                    &readers,
                    restricted(),
                    "Alien",
                    PageRequest::first(1),
                    Some(TitleKind::Show)
                )
                .await
                .unwrap()
            ),
            vec![show]
        );
        // The two empty answers the filter must not turn into a query with nothing to match: a
        // query FTS has no term for, and an empty one, which is not a search at all.
        let unmatched = search_kind(
            &readers,
            restricted(),
            "missing",
            PageRequest::first(1),
            Some(TitleKind::Show),
        )
        .await
        .unwrap();
        assert!(
            unmatched.is_empty(),
            "a term no title holds is an empty page: {unmatched:?}"
        );
        let blank = search_kind(
            &readers,
            restricted(),
            "",
            PageRequest::first(1),
            Some(TitleKind::Movie),
        )
        .await
        .unwrap();
        assert!(
            blank.is_empty(),
            "an empty query is not a search: {blank:?}"
        );
    }
}
