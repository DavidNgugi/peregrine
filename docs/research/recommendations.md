# Research — "More like this" without machine learning

**Audience:** whoever implements or reviews the title-page recommendation rail (`PG-E7-15`,
`PG-E7-08`), and whoever maintains `docs/research/data-sources.md`.
**Why this file exists:** `data-sources.md` records *what each provider offers*; this records
*what to compute from it and how*, which no existing document carries. It is research input, not a
decision — the decisions it recommends belong in an ADR when they are taken.

Every claim about an API or an algorithm carries a URL. Claims marked **[verified]** were read in a
primary source during this research; claims marked **[unverified]** were not, and say so.

---

## 0. The reported bug, diagnosed

A user opens *Avatar* and the rail shows unrelated films. Two independent causes, both confirmed:

1. **The endpoint is the wrong one.** `GET /movie/{id}/similar` is, in TMDB staff's own words, "just
   an aggregation around genres and keywords" over "user added" data, and "as has been the case for
   a number of years now, I have not recommended using similar. If you want more of a MLT type data
   set, I urge you to use `/recommendations` instead. This data is based off of user ratings which is
   better in most situations (although still not perfect, especially for new releases)."
   Sources: <https://www.themoviedb.org/talk/5b70fd3fc3a368189916a57f?language=en> and
   <https://www.themoviedb.org/talk/610c41b55bce9e002cf84757?language=en>. **[verified]** The second
   thread is the same failure as ours, with a different film: a user reports that
   `/movie/299534/similar` (*Avengers: Endgame*) returns "Chicken Little, Home Alone 2, The Crow,
   Click" where it used to return "Avengers, Avengers 2, Captain America, Iron Man".
   TMDB staff's 2023 comment on the sibling bug: "the similar methods are basically EOL'd at this
   point" — <https://www.themoviedb.org/talk/62f77cd9f92532007e9a0d2d?language=en>. **[verified]**
2. **No signal in either endpoint means "same franchise".** Genres and keywords cannot express
   *Avatar → Avatar: The Way of Water*: both are `Adventure|Action|Fantasy|Science Fiction`, shared
   with thousands of films, and the newest part has almost no ratings for the rating-based endpoint
   to have learned from. The one exact signal is TMDB's **collection** — and it was being ignored.

The fix is therefore not a better similarity function. It is: **a hard, non-negotiable franchise tier
placed above every similarity score**, then a ranked rest.

---

## 1. TMDB specifics

### 1.1 `/recommendations` vs `/similar`

| | `/movie/{id}/recommendations`, `/tv/{id}/recommendations` | `/movie/{id}/similar`, `/tv/{id}/similar` |
| --- | --- | --- |
| Documented description | *(empty in the OpenAPI spec)* | "Get the similar movies based on genres and keywords." |
| What it is computed from | user ratings / co-rating behaviour (TMDB staff) | genres + keywords only (TMDB staff and the reference page) |
| Documented caveat | none | "This method only looks for other items based on genres and plot keywords. As such, the results found here are not always going to be :100:. Use it with that in mind." |
| Query parameters | `language`, `page` | `language`, `page` |
| Staff recommendation | "I almost exclusively recommend using recommendations instead" | "does not tend to yield very good results"; "basically EOL'd" |
| Self-inclusion | not reported | **returns the queried item as the first result** |

Sources, all **[verified]**:
`https://developer.themoviedb.org/reference/movie-recommendations.md`,
`https://developer.themoviedb.org/reference/movie-similar.md`,
`https://developer.themoviedb.org/reference/tv-series-recommendations.md`,
`https://developer.themoviedb.org/reference/tv-series-similar.md`,
the two TMDB talk threads in §0.

**Which is better, and why.** `/recommendations`, with `/similar` as a fallback, which is what this
repository's adapter already does (`crates/peregrine-providers/src/tmdb.rs`, `recommendations_url` /
`similar_url`). Neither is sufficient on its own and neither is a franchise signal. `/recommendations`
is better *on average* because co-rating carries information that genres and keywords do not, but it
is **worse exactly where our bug is**: a brand-new or low-vote release, where there are no ratings to
learn from — TMDB staff say so explicitly for new releases. That asymmetry is the reason the
collection tier must not be a similarity signal with a large weight; it must be a tier.

**Two response-shape hazards, both confirmed:**

- The appended key can be **absent, not empty**. A real client crashed on
  `KeyError: 'recommendations'` because the code assumed `response["recommendations"]` always exists;
  the fix was `response.get("recommendations", {}).get("results", [])`. The field was missing for
  *Ice Age: The Meltdown* (tmdb 950). Source:
  <https://github.com/FuzzyGrim/Yamtrack/issues/670>. **[verified]** Decode it as `Option` and treat
  absent and empty identically.
- `/similar` includes the seed. Trakt hit this in production and traced it to TMDB: "Looks like TMDB
  is returning the movie in their similar movies API call … the first movie in that response is the
  same movie." Source: <https://forums.trakt.tv/t/movie-recommends-itself/10741>. **[verified]**
  Filter the seed out of **both** endpoints by provider id, and re-filter after mapping to local rows.

### 1.2 `belongs_to_collection` and `/collection/{id}`

`GET /movie/{id}` carries, among its documented top-level keys **[verified]**:

```json
"belongs_to_collection": {
  "id": 10, "name": "Star Wars Collection",
  "poster_path": "/pWVLFh4OuejTpUaDQbB1C4zoS2p.jpg",
  "backdrop_path": "/iY2ujEY2m68OTTlPFiHub9joHS.jpg"
}
```

It is `null` when the film is in no collection. It is **movie-only**, confirmed three ways
**[verified]**: the key is present in the documented `/movie/{id}` example and absent from the
documented `/tv/{series_id}` example; TMDB's reference lists no TV collection endpoint (the nav has
`COLLECTIONS: Details | Images | Translations` and no TV counterpart —
`https://developer.themoviedb.org/reference/collection-details.md`); and franchises are modelled as
film collections.

`GET /collection/{collection_id}` — parameters are only `collection_id` and `language`
**[verified]**:

```json
{
  "id": 10, "name": "Star Wars Collection",
  "original_language": "en", "original_name": "Star Wars Collection",
  "overview": "An epic space-opera theatrical film series, …",
  "poster_path": "/22dj38IckjzEEUZwN1tPU5VJ1qq.jpg",
  "backdrop_path": "/4z9ijhgEthfRHShoOvMaBlpciXS.jpg",
  "parts": [
    { "adult": false, "backdrop_path": "/2w4xG178RpB4MDAIfTkqAuSJzec.jpg",
      "id": 11, "name": "Star Wars", "original_name": "Star Wars",
      "overview": "…", "poster_path": "/6FfCtAuVAW8XJjZ7eWeLibRLWTw.jpg",
      "media_type": "movie", "original_language": "en",
      "genre_ids": [12, 28, 878], "popularity": 15.8557,
      "release_date": "1977-05-25", "video": false,
      "vote_average": 8.205, "vote_count": 21522 }
  ]
}
```

Three shape facts that a decoder gets wrong:

1. **A part is named `name`, not `title`** — and carries `original_name`, `media_type`, `genre_ids`,
   `popularity`, `video`, `adult` that a search result does not. `parts` are *mixed-type* list
   objects. [`SearchResult` in the adapter already reads both `title` and `name`, so it is safe — but
   the test fixture only exercises `title`, and should exercise `name` too.]
2. **The ordering of `parts` is not documented as guaranteed.** The Star Wars example happens to be
   release-ordered. Sort locally by `release_date`, undated last, so the rail cannot inherit an
   undocumented order.
3. There is no `total_pages`; a collection is returned whole.

**Detecting a franchise for a TV show.** There is no equivalent, and no substitute that is exact.
Be explicit about this rather than inventing one:

- `belongs_to_collection` — does not exist for TV. **[verified]**
- `/tv/{id}/episode_groups` — **not** a franchise mechanism. It returns alternate *ordering schemes
  within one series* (`https://developer.themoviedb.org/reference/tv-series-episode-groups.md`); the
  repo already uses it that way (episode ordering per scheme, `PG-E4-10`).
- `networks` — a network is a brand, not a franchise (HBO carries unrelated shows).
- `created_by` — the best available signal. A creator with several shows is a real franchise
  (`created_by` is in the documented TV details response). It is a *heuristic*, so it belongs in a
  lower, differently-labelled tier: "More from {creator}", never "Same collection".
- `production_companies` — `Marvel Studios`, `Lucasfilm`, `Bad Robot` are franchise-ish; `Warner
  Bros.` is not. Use only with corroboration.
- Name prefix — see §5.1; unsafe alone.

So: **for movies, an exact franchise tier; for shows, a labelled heuristic tier.** Do not present the
TV heuristic with the same confidence as the movie collection.

### 1.3 `/discover/movie` and `/discover/tv`

Full parameter set and the `sort_by` enums are in the official OpenAPI specs, fetched as
`https://developer.themoviedb.org/reference/discover-movie.md` and
`https://developer.themoviedb.org/reference/discover-tv.md`. **[verified]**

`/discover/movie` `sort_by` enum **[verified from the spec]**:
`original_title.asc|desc`, `popularity.asc|desc`, `revenue.asc|desc`,
`primary_release_date.asc|desc`, `title.asc|desc`, `vote_average.asc|desc`,
`vote_count.asc|desc`. Default `popularity.desc`.
Note: `release_date.asc|desc` is listed by an old community wrapper
(<https://search.r-project.org/CRAN/refmans/TMDb/html/discover_movie.html>) and is **not** in the
spec — do not send it.

`/discover/tv` `sort_by` enum **[verified from the spec]**:
`first_air_date.asc|desc`, `name.asc|desc`, `original_name.asc|desc`, `popularity.asc|desc`,
`vote_average.asc|desc`, `vote_count.asc|desc`. Default `popularity.desc`.
The two vocabularies do not overlap on the date field, which is why the adapter's order mapping is
total in `(order, kind)` — that design is correct and should be kept.

Filters relevant to a "more like this" rail **[verified from the specs]**:

| Parameter | Kind | Notes |
| --- | --- | --- |
| `with_genres` | both | genre **ids**; comma = AND, pipe = OR |
| `with_keywords` | both | keyword **ids**; comma = AND, pipe = OR |
| `without_genres` | both | no AND/OR language documented |
| `without_keywords` | both | |
| `with_original_language` | both | ISO 639-1 |
| `primary_release_date.gte` / `.lte` | movie | `YYYY-MM-DD` |
| `first_air_date.gte` / `.lte` | TV | `YYYY-MM-DD`; `air_date.gte/.lte` also exist |
| `vote_average.gte` / `.lte` | both | float |
| `vote_count.gte` / `.lte` | both | integer |
| `with_runtime.gte` / `.lte` | both | minutes |
| `with_companies` / `without_companies` | both | |
| `with_cast`, `with_crew`, `with_people` | movie | person ids |
| `with_networks` | TV | network ids |
| `with_status` | TV | `[0..5]`, comma/pipe |
| `with_type` | TV | `[0..6]`, comma/pipe; `0`…`6` map to the `type` field |
| `page`, `language`, `include_adult`, `region`, `year`, `primary_release_year` | | |

**AND/OR syntax — documented verbatim:** "a number of filters support being comma (`,`) or pipe
(`|`) separated. Comma's are treated like an `AND` query while pipe's are treated like an `OR`."
**[verified]** This is the single most useful lever for this feature: `with_keywords=a|b|c` is an
*any-of* filter, which is what a content-similarity query wants, whereas `a,b,c` is an *all-of*
filter that will usually return nothing for a title with five keywords. **Use `|`, never `,`, for
this feature.** (A `,` query is correct only when you deliberately want "must have all of these".)

**Page caps.** A TMDB moderator states: "There will always be up to **20 items per page**. And a
**maximum** of **500 pages**. Therefore, **at most 10,000 items**" for search/discover, and 1,000
pages for trending. Source: <https://www.themoviedb.org/talk/62ee92a99369a2007dc23ffa?language=en>.
**[verified as a staff-forum statement; not in the official reference]** The same page also says
requests beyond the cap "return a validation error, rather than being silently clamped"
(<https://github.com/magnus919/agent-skills/blob/main/tmdb/references/auth-pagination-and-errors.md>,
a third-party summary — **[unverified]**). Conflicting older wrapper docs claim `page` max is
**1000** (<https://search.r-project.org/CRAN/refmans/TMDb/html/discover_movie.html>). Treat **500** as
the operational ceiling; the disagreement is moot here because a rail reads page 1 only.
The provider adapter's comment "TMDB documents 500 pages" overstates the source — say "TMDB's
support forum states 500" or drop the citation.

**Other caps:** `append_to_response` takes "comma separated list of endpoints within this namespace,
**20 items max**" **[verified from the spec]**, and the response keys mirror the tokens, including a
slash that must be percent-encoded (`watch%2Fproviders` →
`https://developer.themoviedb.org/docs/append-to-response.md`).

### 1.4 Keywords endpoints — the response keys differ

`GET /movie/{id}/keywords` **[verified from the spec example]**:

```json
{"id": 550, "keywords": [{"id": 818, "name": "based on novel or book"}, …]}
```

`GET /tv/{id}/keywords` — note the path is **`/tv/{series_id}/keywords`** and the collection key is
**`results`, not `keywords`** **[verified from the spec example]**:

```json
{"id": 1399, "results": [{"name": "based on novel or book", "id": 818}, …]}
```

Source: `https://developer.themoviedb.org/reference/movie-keywords.md` and
`https://developer.themoviedb.org/reference/tv-series-keywords.md`. A shared decoder that reads
`keywords` for both silently returns zero keywords for every show. **Do not write one decoder.**

The local catalogue already stores keywords in `title_keyword`, so this endpoint is only needed for
the seed title when its keywords are missing — and it can be folded into the details request with
`append_to_response=keywords`, costing zero extra requests when the details are being read anyway.

### 1.5 Rate limits and politeness

Official guidance, verbatim:

> "As of December 16, 2019, we have disabled the original API rate limiting (40 requests every 10
> seconds.) … While our legacy rate limits have been disabled for some time, we do still have some
> upper limits to help mitigate needlessly high bulk scraping. They sit somewhere in the 40 requests
> per second range. This limit could change at any time so be respectful of the service we have built
> and respect the `429` if you receive one."
> — <https://developer.themoviedb.org/docs/rate-limiting> **[verified]**

Consequences for this feature, all of which the existing ADR 0008 machinery already provides:

- **Budget in requests per title view, not in aggregate.** See §4.4.
- **Conditional requests everywhere.** A `304` on a recommendation list costs no body; the repo does
  this already, and ADR 0007 rule 6 makes it an architectural rule.
- **Back off on `429` with `Retry-After`**, per `429`-aware token buckets (ADR 0008 rule 4).
  `X-RateLimit-Limit/Remaining/Reset` are reported by third parties as present
  (<https://github.com/magnus919/agent-skills/blob/main/tmdb/references/auth-pagination-and-errors.md>)
  — **[unverified]**; do not depend on them, but log them if seen.
- **Never page `/discover` in a loop** for a rail. One page, and only on a miss.
- The `429` was observed live during this research against `developer.themoviedb.org` itself when
  fetching documentation in a burst — the limit is enforced, and it is enforced against
  documentation reads too.

### 1.6 Caching ceilings that constrain the design

TMDB's terms require cached content to be refreshed or purged within six months and deleted on
termination (recorded in `docs/research/data-sources.md` §1.1 and ADR 0007 rule 9). This is
comfortable for collection membership (which changes on the scale of years) and for recommendation
lists (which change on the scale of weeks), and uncomfortable for nothing here.

---

## 2. Non-ML techniques, with the math

Notation. For a seed title `S` and candidate `C`: `G` = genre id set, `K` = keyword id set,
`P` = people (director/creator + top-billed cast), `N` = the number of catalogued titles of the same
`kind` that carry the attribute in question.

Every signal below is **local** — it needs no network and no training. That is the whole point: the
only thing the provider is genuinely needed for is the franchise and the co-rating list.

### 2.1 Genres — IDF-weighted, query-normalised overlap

Plain Jaccard is `|G_S ∩ G_C| / |G_S ∪ G_C|`
(<https://en.wikipedia.org/wiki/Jaccard_index>). It is symmetric and it is wrong here, for two
reasons: a candidate with eleven genres should not be punished for its breadth, and *Drama*
(≈ 40 % of TMDB) should not count the same as *TV Movie*.

Use a **query-normalised IDF-weighted overlap**:

```
w(a)      = ln(1 + N / n_a)                       // a genre or keyword id
sim(S, C) = Σ_{a ∈ A_S ∩ A_C} w(a) / Σ_{a ∈ A_S} w(a)        ∈ [0, 1]
```

This is asymmetric on purpose — it answers "how much of what makes the seed itself is present in the
candidate", which is the question a rail asks. `ln(1 + N/n)` is a standard smoothed inverse document
frequency (<https://en.wikipedia.org/wiki/Tf–idf>; the same shape as the IDF in
<https://en.wikipedia.org/wiki/Okapi_BM25>).

- `N` counts titles of the same `kind` **that have at least one genre**, read from `title_genre`.
- Guard: if `|G_S| < 2`, the genre signal is too weak to order by — phase it down (see §2.9).
- Genre ids are **per-kind**. TMDB numbers movie and TV genres differently even when the names
  match; joining across kinds is a real bug class. The adapter already refuses to do it
  (`MetadataProvider::genres` takes a `TitleKind`).

### 2.2 Keywords — the heaviest signal, and the one that needs clipping

Same formula over `K`, and the IDF spread is what makes it the strongest signal: TMDB's most common
keywords appear in tens of thousands of titles and the rarest in one, so `w` spans roughly two orders
of magnitude where genre `w` spans one.

Two guards, both necessary:

1. **Clip per-keyword weight** at a high percentile (or at `ln(1 + N)`), so one
   once-in-the-database keyword cannot single-handedly decide the order.
2. **Cap the contribution of a small stop-keyword list.** TMDB's most frequent keywords are
   mood/credit boilerplate that cross every genre: `aftercreditsstinger`, `duringcreditsstinger`,
   `based on novel or book`, `woman director`, `revenge`, `murder`, `anime`. Inverse document
   frequency already discounts them; an explicit list makes the *reason string* honest, because
   "Similar themes: aftercreditsstinger" is worse than saying nothing.
3. **Require `|K_S ∩ K_C| ≥ 2`** before keyword overlap may be reported as a reason, even when one
   shared rare keyword produces a high score. One keyword is a coincidence.

### 2.3 Overview text — TF-IDF cosine, or FTS5 BM25

**Cosine over TF-IDF vectors.** Term frequency × inverse document frequency over the tokenized,
stop-worded, stemmed overviews of the local catalogue; similarity is the cosine of the two vectors
(<https://en.wikipedia.org/wiki/Cosine_similarity>, <https://en.wikipedia.org/wiki/Tf–idf>, weight
scheme after Salton & Buckley, *Term-weighting approaches in automatic text retrieval*, 1988,
<https://doi.org/10.1016/0306-4573(88)90021-0>). Practical notes:

- **Tokenize with the same tokenizer the search index already uses** — `unicode61 remove_diacritics 2`
  — so "Amélie" and "Amelie" agree, and so there is one tokenization behaviour in the codebase.
- **Stop words:** use a published list rather than an invented one. The Snowball stop lists are
  permissively licensed and widely used (<https://snowballstem.org/algorithms/>,
  <https://github.com/snowballstem/snowball/tree/master/algorithms>); NLTK's English list is the other
  common choice. Whichever is used, it must be **committed and versioned**, not fetched, so
  behaviour is reproducible offline (R14).
- **Build the IDF table over the local catalogue only.** It must not depend on the provider, or the
  feature stops working offline and the scores change when the catalogue changes size. Recompute it
  on the maintenance job that already rebuilds the search index.
- **Overviews are long and share boilerplate.** Cap at ~60 tokens per overview, or the signal
  degenerates into "everything is 0.05 similar to everything".

**The cheaper path, and the one to try first.** `title_fts` already indexes `overview` and
`keywords` for the whole catalogue, and SQLite FTS5 ships a `bm25()` ranking function whose
implementation is documented exactly: `k1` and `b` are **hard-coded at 1.2 and 0.75**, IDF is
`ln((N - n(q_i) + 0.5)/(n(q_i) + 0.5))`-shaped, and the returned value is **multiplied by −1 so that
better matches are numerically lower**
(<https://sqlite.org/fts5.html>, §5.1.1). **[verified]**

So "similar overview + keywords" is a *document-as-query* query — Lucene calls this MoreLikeThis, and
the recipe is the same one Lucene's `MoreLikeThis` uses (take the top-N terms of the source document
by TF-IDF, build a query from them, rank by BM25;
<https://apache.googlesource.com/lucenenet/+/d5a5e9839efe2715e49b748159593926747463da/src/Lucene.Net.Queries/Mlt/MoreLikeThis.cs>):

```sql
-- sketch, not production code
SELECT rowid, -bm25(title_fts, 0.0, 0.0, 0.0, 1.0, 2.0, 0.0) AS score
FROM   title_fts
WHERE  title_fts MATCH :seed_terms          -- OR of the seed's top ~8 terms, quoted
ORDER  BY score DESC
LIMIT  200;
```

Column order is `name, original_name, aliases, overview, keywords, cast_names`, so the weights above
score `overview` at 1.0 and `keywords` at 2.0 and ignore the rest. **Two verified prerequisites**:

- **`bm25()` needs the document-length table, which a contentless table still has — unless
  `columnsize=0` is set.** SQLite: "Normally, FTS5 maintains a special backing table … used by the
  built-in bm25 ranking function"; with `columnsize=0` on a *contentless* table, "The xColumnSize API
  always returns -1. There is no way to determine the number of tokens in a value stored within a
  contentless FTS5 table configured with columnsize=0."
  — <https://sqlite.org/fts5.html> §4.5. **[verified]** `title_fts` is created `content=''`,
  `contentless_delete=1` and does **not** pass `columnsize`, so it defaults to `1` and the
  `title_fts_docsize` table exists. **BM25 works, and a future migration that adds `columnsize=0` to
  save space would silently break this feature.** That trade must be recorded next to the option.
- **`contentless_delete=1` leaves tombstones until a merge** (<https://sqlite.org/fts5.html> §6.5).
  `N` and `n(q_i)` in FTS5's IDF may therefore briefly count deleted rows. Harmless for a rail;
  worth a comment so nobody chases it.

Choose TF-IDF cosine when the ranking must be *stable and inspectable* (it is, and it can be
reproduced in a test without SQLite), and FTS5 BM25 when the budget matters (it is one indexed query
over a table that already exists). They are close enough in quality that the deciding factor is which
one the team can test. The FTS5 path is recommended as the first attempt, with a golden-fixture test
that pins the order.

### 2.4 Release-era proximity

Use a **plateau, not a decay from zero**, so "same era" is a statement rather than a slope:

```
era(S, C) = 1                                   if |Δyear| ≤ 3
          = exp(−(|Δyear| − 3) / 8)             otherwise
```

`Δyear` from `release_date` (movie) or `first_air_date` (show). Plateau width 3 years, τ 8, gives
1.00 within 3 years, 0.53 at 8, 0.29 at 13, 0.07 at 25. Missing dates → the signal is *absent*, not 0
(see §2.9). Rationale for the plateau: a viewer looking at a 1977 film is not looking for "films from
1976", and an exponential with no plateau makes every old film equally distant, which is the same as
no signal.

### 2.5 Runtime proximity

Same shape, on `runtime_minutes` (movie) or `episode_runtime_minutes` (show):

```
runtime(S, C) = 1                              if |Δmin| ≤ 10
              = exp(−(|Δmin| − 10) / 25)       otherwise
```

1.00 within 10 minutes, 0.45 at 30, 0.14 at 60. Missing on either side → absent, **not** 0: a film
TMDB has no runtime for must not be punished for the provider's omission. For a show, compare episode
runtime, never total running time.

### 2.6 Original language

A soft signal, never a filter:

```
language(S, C) = 1.00   same original_language
               = 0.35   both present and different
               = 0.50   either absent
```

Not a filter because a hard `with_original_language` filter is exactly what makes a rail feel
parochial to a viewer who watches anime, Nordic noir, or Korean cinema. Not 0 for a mismatch because
a remake or a co-production across languages is a legitimate recommendation. `original_language` is
BCP-47 in the schema (`title.original_language`), so compare the language subtag, not the whole tag.

### 2.7 Same director / creator / cast / company

Read from `credit` (`kind`, `job`, `department`, `order_index`, `episode_count`) and `title_company`.

```
director(S, C) = 1  if the two share a person credited job='Director' (movie)
                     or job='Creator' / department='Writing' + creator (show), else 0
top_cast(S, C) = min(1, |top10(S) ∩ top10(C)| / 5)      // top10 by order_index
company(S, C)  = min(1, |co(S) ∩ co(C)| / max(1, |co(S)|))
people(S, C)   = 0.70·director + 0.20·top_cast + 0.10·company
```

`/5` rather than `/10` because sharing five top-billed people is already an extremely strong signal —
a franchise, a repertory company, or a director's stock troupe. Note `credit.person_id` is indexed
(`credit_person_idx`), so "who else did this person work on" is cheap.

### 2.8 Ratings and popularity as a tiebreaker, without the 10.0-from-3-votes problem

**The problem.** `vote_average` is a raw mean. TMDB's own order mapping for score sorts by the raw
average, so "the unfiltered page is a wall of `10.0` from one vote" — the repository already knows
this (`RATING_VOTE_FLOOR = 200` in `crates/peregrine-providers/src/tmdb.rs`, applied whenever a
browse sorts by `vote_average`). That floor is right for a *browse* grid and wrong for a *tiebreak*
inside a rail: it discards the long tail entirely instead of ranking it fairly.

**Bayesian shrinkage toward a prior mean** is the right tool for a tiebreak. It is the IMDb-style
weighted rating:

```
wr = (v·R + m·C) / (v + m)
```

`R` = the item's mean (`tmdb_rating`, 0–10), `v` = `tmdb_votes`, `m` = prior weight in votes,
`C` = prior mean vote. Equivalently, the item's own mean receives weight `v/(v+m)` and the prior
receives `m/(v+m)`. It is the Bayesian average
(<https://en.wikipedia.org/wiki/Bayesian_average>); the formulation is the one IMDb's Top 250 has
used publicly.

Worked numbers with `m = 400`, `C = 6.9`:

| | R | v | wr |
| --- | --- | --- | --- |
| obscure "perfect" film | 10.0 | 3 | 6.92 |
| good, well voted | 8.20 | 25 000 | 8.17 |
| solid, small sample | 8.60 | 120 | 7.29 |

The obscure 10.0 lands *below the prior* and well below a well-voted 8.2, which is the desired
behaviour, and it still beats a 3.0.

**Choosing `m` and `C`.** `m` is "the vote count at which the item's own mean gets half the weight".
`m = 400` is a reasonable start for a mixed catalogue; higher `m` is more conservative. `C` should be
**measured from the local catalogue** when it holds at least ~1 000 rated titles of that kind
(vote-weighted mean of `tmdb_rating`), and fall back to a constant otherwise, so a small catalogue
does not produce a nonsensical prior. For shows, use the show's own votes; do not aggregate episode
votes.

**The Wilson alternative, and why it is the second choice.** The lower bound of the Wilson score
interval (<https://en.wikipedia.org/wiki/Binomial_proportion_confidence_interval>, Wilson 1927;
popularised for ranking by <https://www.evanmiller.org/how-not-to-sort-by-average-rating.html>):

```
p̂ = R/10,  n = v,  z = 1.96
lower = ( p̂ + z²/(2n) − z·√( (p̂(1−p̂) + z²/(4n)) / n ) ) / ( 1 + z²/n )
```

For `R=10, v=3` this gives **0.233** (i.e. 2.3/10) and for `R=8.2, v=25 000` it gives **0.815** — a
stronger separation than the Bayesian form, and it needs no corpus mean `C`. The reason to prefer
the Bayesian form anyway: Wilson is a *lower bound*, so it is negatively biased everywhere and
systematically penalises low-vote items relative to their true quality. Miller's post itself has the
SQL, and a well-known critique argues Laplace/Bayesian smoothing is "as good or better" and needs
no square roots (<https://raw.githubusercontent.com/ajschumacher/ajschumacher.github.io/0437a64db7ea740ac79507e0be441ab1485f8fcb/2014/08/17/how-to-sort-by-average-rating/index.md>).
For a **tiebreak among already-similar titles**, shrinking toward the corpus mean is the fairer
behaviour; use Wilson only if a test shows it orders the golden set better.

**Popularity last.** `popularity` is a lifetime score built from daily votes, views, favourites,
watchlist adds, release date, total votes, and the previous day's score
(<https://developer.themoviedb.org/docs/popularity-and-trending>). **[verified]** It is useful as the
final deterministic tiebreak and as a cold-start filler, and it is the wrong primary signal: it is
driven by TMDB's user base, which is heavily English-language, so ranking by it bakes that bias into
every rail (§5.8).

### 2.9 Combining signals: weighted linear blend vs rank fusion

**Weighted linear blend (WLC).**

```
local(S, C) = Σ_i w_i · s_i(C)  /  Σ_{i ∈ present} w_i
```

with `present` = the signals for which *both* sides have data. Normalising by the present weight
rather than the total is not a detail: normalising by the total silently punishes a title whose
provider record happens to lack a runtime, which produces "this film is recommended for no reason"
bugs that are very hard to see. Add a **coverage floor**: if the present weight is below ~0.45, the
local score is not trusted and the candidate may not enter the strong tier.

Starting weights, summing to 1.00:

| Signal | Weight | Why this weight |
| --- | --- | --- |
| keyword (IDF overlap) | 0.30 | highest-information, widest IDF spread, most explainable |
| genre (IDF overlap) | 0.20 | coarse but never empty, so it carries the base |
| overview / FTS BM25 | 0.15 | real but noisy; overlaps heavily with keywords |
| people (director/cast/company) | 0.17 | strongest *single* evidence when it fires, sparse |
| era | 0.08 | a preference, not a similarity |
| runtime | 0.06 | the weakest real signal |
| language | 0.04 | deliberately small — see §2.6 |

Properties: magnitudes are comparable, weights are interpretable, and a per-signal contribution
`w_i·s_i` falls out for free, which is what the reason strings need. Costs: it needs calibration, and
a signal that is systematically smaller for some slice of the catalogue (anime overviews, silent
films with no cast) biases against that slice.

**Reciprocal Rank Fusion (RRF).** Produce one ranked list per signal, then

```
RRF(d) = Σ_{r ∈ R} 1 / (k + rank_r(d)),      k = 60
```

The formula and `k = 60` are from Cormack, Clarke & Buettcher, *Reciprocal Rank Fusion outperforms
Condorcet and individual Rank Learning Methods*, SIGIR 2009, p. 1:
<https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf> — "we compute RRFscore(d ∈ D) = Σ_{r ∈ R}
1/(k + r(d)), where k = 60 was fixed during a pilot investigation and not altered during subsequent
validation", and "the choice was not critical". **[verified]** A readable walkthrough of RRF, Borda
and the alternatives is <https://raw.githubusercontent.com/izikeros/blog/3d6e1b08b7dad1a61d0721f89ef094af20f7db19/content/posts/notes/rank_fusion_algorithms.md>.

Properties: **no tuning at all**, robust to one bad signal, and — importantly — a candidate absent
from a list simply contributes nothing, so missing data degrades gracefully instead of needing
renormalisation. Costs: it **throws away magnitude**, so a candidate that is 10× better on genre
overlap ties with one that is barely better, and it cannot express "genre matters more than runtime"
without reintroducing a weight (`w_r/(k+rank_r)`), at which point it is a linear blend in rank space.

**Borda count**, for completeness: `Borda(i) = Σ_j (m − rank_j(i))`
(<https://en.wikipedia.org/wiki/Borda_count>, and the same write-up above). It is what you get when
you rank *tiers* rather than items, which is exactly the shape this feature wants.

**Recommendation: do not flatten.** Use a **Borda-style tier order** as the primary ordering and a
**weighted linear blend inside the local tier**. Reasons, in order of importance:

1. The franchise requirement is a *constraint*, not a preference. Expressing "must come first" in a
   linear blend needs an arbitrarily large weight, which is a tier with extra arithmetic and less
   clarity.
2. The provider's list has **no score at all** — `/recommendations` returns results, not a relevance
   number. Fusing it linearly with a `[0,1]` local score means inventing a scale for it.
3. RRF is the right tool only when the thing being fused genuinely has no common scale — which is
   exactly the case for `{local content score} × {provider list} × {watch history}`, so keep it in
   reserve for the day the rail is flattened into a single scalar for paging. Until then, tiers are
   simpler, testable, and explainable.

**Tuning without ground truth.** There is no labelled relevance set, and pretending otherwise is the
usual failure mode. Four things that actually work, in order:

1. **A franchise hold-out, as a real metric.** For every title that belongs to a collection, hide one
   part and measure whether the hidden part appears in the top *k* — MRR@10 and recall@1. Deterministic,
   offline, no labels, and it measures the exact bug that was reported. Its bias must be stated: it
   only measures franchise recovery and says nothing about the rest of the rail.
2. **A committed golden fixture.** ~30 seed titles across the interesting shapes (a franchise, a
   one-off with a rich keyword set, an anime series, a documentary, a title with no keywords, a title
   with no provider id, a non-English title, a remake pair, a title with two votes) with an expected
   first five and a pinned ordering. This is the repository's existing idiom (`docs/DATA_MODEL.md`
   names invariants after the tests that enforce them; R4/R13/R14). The fixture is the acceptance
   test for any weight change.
3. **Weight-perturbation stability.** Perturb every weight by ±20 % and measure Kendall's τ against
   the baseline ranking on the golden fixture. Keep the weights that survive; if no reasonable weight
   vector produces a stable order on a seed title, the *signal set* is wrong for that title, not the
   weights. This is the cheapest defence against overfitting seven numbers to thirty examples.
4. **Coverage and empty-rate telemetry, local only.** What fraction of title views produce a rail of
   ≥ 8 items, and what fraction of rails are entirely Tier 3/4. No telemetry leaves the machine
   (R23); this is a local diagnostic, surfaced in Diagnostics, not an upload.

Explicitly *not* recommended: fitting the weights to `popularity` or to `vote_count` as a proxy for
relevance. That optimises toward "recommend what is already popular", which is the behaviour the rail
exists to avoid.

### 2.10 Guaranteeing "same franchise first", deterministically

```
tier(S, C) =
  0   C ∈ collection(S)                                 // movies; ordered by release_date
  1   local(S, C) ≥ θ_strong  (0.35)  and coverage ≥ 0.45
  1   C ∈ {same created_by(S)}                          // TV only, labelled differently
  2   C ∈ recommendations(S)                            // provider's own order
  3   local(S, C) ≥ θ_min  (0.12)
  4   C ∈ discoverPool(S)                               // only if the rail is still short
```

Deterministic total order inside the rail:

```
ORDER BY tier ASC,
         within_tier_score DESC,
         wr DESC,            -- the Bayesian tiebreak of §2.8
         tmdb_votes DESC,
         title_id ASC        -- the only reason two runs ever agree
```

`title_id` last is not decoration: without a total order, two candidates with equal scores swap on
every run and every test that pins an order becomes flaky. The repository already has this rule
(`xtask check-determinism`; R14).

The seed is excluded at the pool stage by `t.id <> :seed`, and again after mapping, because a
`/similar` result can carry the seed's provider id.

Tiers 1 and 3 are the same signal at two thresholds; the split exists so that "similar" and
"vaguely similar" can be ordered against the provider's list without either being dropped.

### 2.11 Explainability

Each signal produces a contribution `w_i·s_i`, renormalised by the present weight — so a reason is
*derived from the same arithmetic that produced the ranking*, not attached afterwards. Take the top
two contributions that clear a floor (0.08, i.e. 8 % of the present weight), map each to a string,
and cap at two. If nothing clears the floor, show **no reason**; "Recommended for you" is noise.

| Source | Condition | String |
| --- | --- | --- |
| tier 0 | always | "Part of {collection name}" (needs §4 S1 for the name), else "Same collection" |
| tier 1, TV creator | creator matches | "More from {name}" |
| genre | top-2 contribution, `|G_S ∩ G_C| ≥ 2` | "Shares {g1} and {g2}" |
| keyword | `|K_S ∩ K_C| ≥ 2` | "Similar themes: {k1}, {k2}" |
| text | BM25/cosine is a top-2 contribution | "Similar story" |
| people | director/creator matches | "Directed by {name}" / "Created by {name}" |
| people | top-2 contribution by cast | "Stars {name}" |
| era | `era ≥ 0.8` | "Same era ({decade}s)" |
| language | `language = 1` | "Also in {language name}" — **only** with a second reason, never alone |
| runtime | never | suppress: "similar length" is not a reason anybody acts on |

Two rules that keep this honest:

- **A reason must be true of the pairing, not of the seed.** "Action" is not a reason that *this*
  candidate was chosen for an Action seed.
- **A localised reason string is UI copy and belongs with the other strings.** The core returns a
  typed reason (an enum plus its parameters), never a rendered sentence — this is R7 and R10: no
  user-facing sentence is composed in Rust, and no business rule is composed in TypeScript.

### 2.12 Cold start

Three distinct cold starts, and they need different answers:

| Situation | Answer |
| --- | --- |
| **Empty local catalogue, provider key present** | The rail is the provider's `/recommendations` page 1, plus the collection tier (2–3 requests). There is nothing to compute locally and nothing to compute it against. Degrade to Tier 2 + Tier 0. |
| **Empty local catalogue, no provider key** | There is no rail. Say so once, in the empty state, with a link to Settings — do not fabricate a rail from a handful of stubs, and do not silently hide the section (a rail that vanishes reads as a bug; an explained empty state is a feature). This is the state the app ships in before onboarding. |
| **Rich catalogue, no provider key** | Full local rail: Tiers 0 (if collection membership was stored while a key existed), 1, 3, 4-from-local-genre. This is the case worth designing for, because it is the local-first promise. |
| **No watch history** | Irrelevant here — this rail is seed-based, not history-based. History matters for "Because you watched" (`PG-E7-15`'s other half), which is a *different* rail and should stay one. |
| **A title with no provider id** (inferred from a filename) | Today the runtime answers empty (`Runtime::similar` returns `Vec::new()`). After S3 that becomes a real local rail — this is the single biggest visible win from the local work. |

The `title.metadata_state` column already distinguishes `stub` from `listed`/`partial`/`complete`,
which is what decides whether a local signal can be trusted at all: a `stub` has a name and nothing
else, so it can *be* a candidate but cannot *seed* a content score.

---

## 3. Offline vs provider, and what each option costs

### 3.1 Fully offline (no network)

| Capability | Local source | Cost |
| --- | --- | --- |
| franchise tier (movie) | **needs a new table** — `collection` membership is not stored today (§4 S1) | one local join after S1 |
| genre overlap | `title_genre` | indexed, ~µs–ms |
| keyword overlap | `title_keyword` | indexed, ~µs–ms |
| overview similarity | `title_fts` MATCH + `bm25()` | one indexed FTS query |
| era, runtime, language | `title` columns | free |
| director / cast | `credit` (`credit_title_order_idx`, `credit_person_idx`) | indexed |
| company | `title_company` | indexed |
| rating tiebreak | `tmdb_rating`/`tmdb_votes`, `imdb_rating`/`imdb_votes` | `title_imdb_rating_idx` exists; add an equivalent for the TMDB pair |
| already-watched suppression | `watch_progress` (`watch_progress_continue_idx`) | indexed |
| AccountScope filtering (I37) | applied **in SQL** | free |
| reasons | derived from the same arithmetic | free |

### 3.2 Needs the provider

| Call | Requests | When | Notes |
| --- | --- | --- | --- |
| `/movie|tv/{id}/recommendations` | 1 | every cold title view | the primary list; may be absent |
| `/movie|tv/{id}/similar` | 1 | fallback, only if recommendations is empty | seed-inclusive; filter |
| `/movie/{id}` | 0–1 | movie, to read `belongs_to_collection` | **free if the page's own details read uses `append_to_response`** |
| `/collection/{id}` | 0–1 | movie with a collection | fully cacheable to the 180-day ceiling |
| `/movie|tv/{id}/keywords` | 0–1 | only if the seed's keywords are missing locally | fold into `append_to_response` |
| `/discover/movie|tv` | 0–2 | only on a miss with a thin local pool | one page, never a loop |

### 3.3 Latency — **unmeasured**

No latency numbers for `api.themoviedb.org` were collected in this research, and this document will
not invent any. The design does not depend on a number, because ADR 0008 rule 1 already fixes the
policy: **the UI never waits for the network**. The rail renders from local state (which after S3 is
a real rail, not an empty one), and the provider result only ever fills in. Whatever the RTT is, it
is behind a cache and never on the first paint.

What *was* measured, by SQLite's documentation rather than by us, is that `bm25()` needs the
`_docsize` table and that `title_fts` has it (§2.3). The local query plan should be measured against
`cargo` benches at 20 000 titles before the rail is called done — the repository's rule (R16, and the
`< 100 ms` line in R10) applies, and the plan is unmeasured today.

### 3.4 Requests per title page — the budget

**Accept ≤ 3 on a cold view, 0 on a warm view, and never on the first paint. The common case is 1.**

| Case | Requests (cold) | After cache |
| --- | --- | --- |
| movie with a collection, details already read on this page | 1 (`/recommendations`) + 1 (`/collection/{id}`) = **2** | 1 |
| movie with a collection, details not yet read | + 1 (`/movie/{id}`) = **3** | 1 |
| show, recommendations non-empty | **1** | 0 |
| show, recommendations empty | 2 (`/recommendations` + `/similar`) | 1 |
| either, both empty and pool thin | + 1–2 (`/discover`) = **≤ 4** — and this is the case to cut first | 1 |

Why 3 is comfortably acceptable: TMDB's stated ceiling is "somewhere in the 40 requests per second
range" (§1.5). A user opening a title page roughly every ten seconds generates ~0.3 req/s at worst —
two orders of magnitude under the ceiling, and it is *not* background traffic, so it never competes
with the rails job. The `429` risk in this app is the sync scheduler, not this rail. The real budget
argument is not rate limit, it is **cache hit rate**: `/collection/{id}` is shared by every part of
the franchise, so the second and third *Avatar* pages the user opens cost zero network.

---

## 4. The design, in implementation slices

Each slice is independently shippable, independently testable (`wiremock` + recorded fixtures, no
network — R14), and each one is useful on its own. **The order is the recommendation.**

### S0 — Answer from `/recommendations`, and put the collection first

*Already present in this working tree; verify, land, and keep (see §6).*

- `/movie|tv/{id}/recommendations`; `/similar` only as a fallback when it is empty **or absent**.
- For movies only: read `belongs_to_collection` from `/movie/{id}`, and `/collection/{id}` when set.
- Collection parts first, ordered by `release_date` (undated last); exclude the seed.
- Decode the appended key as `Option` — absent ≠ error.
- **Requests per title view:** 1 (show) / 2 (movie with a collection, details amortised) / 3 (movie,
  details not amortised). **Provider: yes. Local: no.**

Tests that must exist: the seed never appears (from either endpoint); a missing `recommendations` key
is an empty list and not an error; parts decode from `name` **and** from `title`; parts order by
release date with an undated part last; a show never pays for a collection request.

### S1 — Store the collection locally

- Migration: `collection(id, name, overview, poster_path, backdrop_path, provider, fetched_at)` and
  `title_collection(title_id, collection_id, part_position, UNIQUE(title_id, collection_id))`.
- Filled from the S0 reads; `title.metadata_state`/`last_synced_at` already drive when the details
  are re-read, so this rides the existing refresh path.
- Effect: the franchise tier becomes **offline after first sight**, and the collection *name* becomes
  available, which is what turns "Same collection" into "Part of Avatar Collection".
- **Requests per title view:** 0 extra. **Provider: no (amortised). Local: yes, required.**

Tests: round-trip; a collection read that 404s leaves no row and does not fail the title view;
two parts of one franchise share one `collection` row; the 180-day sweep covers `collection`.

Indexes this adds that S3 needs anyway: `title_genre(genre_id, title_id)` and
`title_keyword(keyword_id, title_id)`. The current PKs are `(title_id, …)`, so "which titles have
genre 18" is a full scan. **This omission is what makes the local candidate query expensive today.**

### S2 — Hygiene: exclude, cap, scope

Pure local, and it prevents the most embarrassing failures.

- Exclude the seed (by local row id **and** by provider id).
- Exclude `adult = 1`; exclude candidates the `AccountScope` hides **in SQL** (I37 — a filter after
  ranking leaks existence through the count and the "more" answer).
- Exclude `status IN ('rumored','canceled')` — announcing a cancelled film is worse than silence.
- Exclude genres the seed's account has denied (ADR 0019's allow/deny surface) and titles already
  `watched = 1` for this account, **as a demotion rather than a removal** if the rail would otherwise
  be short.
- Cap the candidate pool (200) and the rail (20, `RAIL_PAGE_LIMIT`).
- **Requests per title view:** 0. **Provider: no. Local: yes.**

Tests: the seed never appears under any provider fixture; a restricted scope sees the same rail as if
the hidden titles did not exist; a rail is never shorter than the pool allows *because* of the scope
filter (the two halves of I37); an all-excluded pool yields an empty rail and not an error.

### S3 — Local content similarity (the slice that makes this local-first)

- Implement §2.1, §2.4, §2.5, §2.6, §2.9 with the starting weights.
- Candidate generation: one SQL query over `title_genre` ∪ `title_keyword` with the reverse indexes
  S1 adds, capped at 200:

```sql
-- sketch, not production code
WITH seed_genre AS (SELECT genre_id   FROM title_genre   WHERE title_id = :seed),
     seed_kw    AS (SELECT keyword_id FROM title_keyword WHERE title_id = :seed)
SELECT t.id,
       COUNT(DISTINCT tg.genre_id)   AS shared_genres,
       COUNT(DISTINCT tk.keyword_id) AS shared_keywords
FROM   title t
LEFT JOIN title_genre   tg ON tg.title_id = t.id AND tg.genre_id   IN (SELECT genre_id   FROM seed_genre)
LEFT JOIN title_keyword tk ON tk.title_id = t.id AND tk.keyword_id IN (SELECT keyword_id FROM seed_kw)
WHERE  t.id <> :seed AND t.kind = :kind
GROUP  BY t.id
HAVING shared_genres > 0 OR shared_keywords >= 2
ORDER  BY shared_keywords * 3 + shared_genres DESC
LIMIT  200;
```

- Order within Tier 1/3 by `local(S, C)`; give a real rail to a title with **no provider id**.
- **Requests per title view:** 0. **Provider: no. Local: yes.** After this slice the feature works
  with no API key at all.

Tests: the golden fixture's ordering (§2.10 item 2); a title with no keywords still ranks by genre;
a missing runtime is *absent* and not a penalty (the score is identical with and without a null
runtime on an otherwise-equal pair); the candidate query is empty for a seed with no genres and no
keywords and the rail degrades rather than erroring; determinism — two runs produce byte-identical
order (the `title_id` tiebreak).

### S4 — Overview text similarity

- FTS5 BM25 document-as-query over `title_fts` (§2.3), folded into the local blend at 0.15.
- Normalise `-bm25` by the maximum in the candidate set.
- **Guard:** require the seed query to be at least 2 terms after stop-wording, or skip the signal
  entirely — a one-term query over `overview` matches everything.
- **Requests per title view:** 0. **Provider: no. Local: yes.**

Tests: a one-term query is skipped, not run; BM25 contributes nothing for a seed with a null
overview; the feature's behaviour is pinned with a fixture so that a future `columnsize=0` migration
(which would silently break `bm25` length normalisation) fails a test rather than degrading quietly.

### S5 — People and company

- `director`/`creator`, `top_cast`, `company` from §2.7, at 0.17 total.
- **Requests per title view:** 0. **Provider: no. Local: yes.**

Tests: a director match outranks a five-genre overlap; shows use `created_by` and movies use
`credit.job = 'Director'`, and neither leaks into the other; a person in the top 10 of one and
position 40 of the other does not count as a cast match.

### S6 — Tiers, reasons, and the DTO

- The tier order of §2.10 as the rail's ordering, `wr` (§2.8) then `tmdb_votes` then `title_id` as
  the tiebreaks.
- A typed reason per item (§2.11), surfaced through IPC, rendered by the UI.
- **Requests per title view:** 0. **Provider: no. Local: yes.**

Tests: tier 0 always precedes tier 1 (a property test over random scores: no tier-1 item ever
outranks a tier-0 item); the order is total and stable; a reason is only attached when its
contribution clears the floor; the reason is a typed value and the core composes no user-facing
sentence (R7).

### S7 — Rated tiebreak and popular filler

- The Bayesian `wr` of §2.8 as the tiebreak, with `m`/`C` from settings with the measured-catalogue
  fallback.
- `discoverPool` (Tier 4) from a **local** top-rated/top-popular query when the rail is short; the
  provider's `/discover` only if the local pool is genuinely empty.
- **Requests per title view:** 0 normally; ≤ 2 on a miss. **Provider: only on a miss. Local: yes.**

Tests: a 10.0 with 3 votes ranks below an 8.2 with 25 000; `m`/`C` fall back to constants on a
catalogue with fewer than 1 000 rated titles; a Tier 4 item never outranks a Tier 1 item.

### S8 — `/discover` expansion for a thin pool

- Only when (local pool < 8) or (the seed has < 3 keywords and no overview).
- `with_genres` with the seed's genres joined by `|`, `with_keywords` with the seed's top keywords
  joined by `|`, `vote_count.gte` = TMDB's `RATING_VOTE_FLOOR` when sorting by score, one page.
- **Requests per title view:** 1–2, and only for the thumbnails of a catalogue. **Provider: yes.
  Local: no.**

Tests: the `|` separator is used for a multi-value filter and `,` never is; `vote_count.gte` is
present whenever `sort_by` is a score sort; a discover miss yields a short rail and not an error; the
expansion runs at most once per title view.

### S9 — "Because you watched" (history-based, a *different* rail)

- The history vector is the recency-weighted mean of the seed vectors of the last *k* watched
  titles; the same signals, same tiers, no new math.
- Local only (`watch_progress`), and it belongs to `PG-E7-15`, not to this rail. Keep them separate:
  a "More like this" rail that quietly changes meaning when history arrives is worse than two rails.

---

## 5. Pitfalls

### 5.1 Sequels sharing a name prefix — do not use the prefix as a positive signal

The tempting fix for *Avatar* is `name LIKE 'Avatar%'`. It is unsafe, and the failure modes are
concrete:

- **False positives:** *Avatar* (2009) vs *Avatar: The Last Airbender* (a different franchise, and a
  show), *Avatars*, *Avatar* (the 2004 Bollywood film), and any title whose localized name merely
  starts with the string.
- **False negatives:** the localized names are different strings. `title.name` is a localized name;
  `Аватар`, `阿凡达`, `아바타` share no prefix with `Avatar`. `title_alias` carries the regional
  variants, and a prefix rule that reads only `name` is a rule that works only in English.
- **Remakes:** *The Saint* (1997) and *The Saint* (2017), *Total Recall* (1990) and *Total Recall*
  (2012), *The Thing* (1982) and *The Thing* (2011) share a *complete* name. A prefix rule cannot
  separate "sequel" from "remake", and the reason string must not claim a franchise that does not
  exist.

Rule: the prefix may be a **tiebreak inside the collection tier** (where membership has already been
proven by the provider, so the prefix adds nothing and costs nothing) or a **corroborating signal in
a separately-labelled tier**, gated on *all* of: normalized `sort_name` prefix match **against both
`name` and `title_alias`**; same `kind`; and at least one of {shared production company, shared
director/creator, shared top-billed cast, release gap ≤ 25 years}. And when it fires it says
"Possibly related", never "Same collection". A wrong-but-hedged label is much better than a
confident false one.

For remakes specifically: two titles with the same normalized name and different `release_date`
years are a remake pair, and a remake is a legitimate recommendation — just never in the franchise
tier.

### 5.2 Self-inclusion

Covered in §1.1 and §2.10. Three failure paths, all needing a guard: `/similar` returns the seed
first; `/recommendations` may or may not; and a stale local row can carry a provider id that now
resolves elsewhere. Filter on the **local `title_id`** after mapping, not only on the provider id
before it.

### 5.3 Multi-language titles, aliases, and normalisation

- `title.name` is localized. `title.original_name` and `title_alias` are the other spellings.
- Normalise with the same tokenizer the FTS index uses — `unicode61 remove_diacritics 2` — and
  compare on the normalized form, or *Amélie* and *Amelie* become different films.
- `sort_name` is the sort key and is what a prefix rule should read (so *The Matrix* sorts as
  *Matrix, The*), but it is not guaranteed to be ASCII.
- CJK titles have no word boundaries; the `unicode61` tokenizer will produce very few, very long
  tokens. Text similarity over CJK overviews is near-useless. For a seed whose `original_language` is
  CJK, weight keywords and people and phase the text signal down. This is a real limitation and
  should be a documented one, not a bug report.

### 5.4 TV seasons and episodes vs shows

- TMDB numbers movies, shows, seasons, **and episodes** in the same integer space. Passing a season
  or episode id to `/tv/{id}/similar` returns another title's data or nothing.
- `external_id` has `season_id` and `episode_id` columns precisely because of this, with a `CHECK`
  that exactly one owner is set. The rail read must select the row **whose `title_id` is set**. The
  existing runtime does this by reading `library_title(...)` and then `external_ids_of(title_id)`,
  which is correct — keep it that way, and keep the test that proves it.
- A rail shown on an **episode page** must resolve to the parent `title_id`. Season-level
  recommendations do not exist in TMDB.
- A show's `similar` is show-level, so a rail on a season page is the show's rail. Pick one
  behaviour and test it rather than letting it fall out of the code path.

### 5.5 Irrelevant anime/animation leakage

- Keywords like `anime`, `cartoon`, `superhero`, `based on manga` are high-IDF but genre-crossing: a
  seed with `anime` pulls every anime regardless of genre, and a seed with `superhero` pulls every
  superhero film across four decades.
- Three mitigations, all cheap: cap the weight of the stop-keyword list (§2.2); **require a minimum
  genre overlap before keyword overlap may carry a candidate** (keyword score is multiplied by a
  factor that goes to 0 when genres do not overlap at all); and use `title.subtype`
  (`standard|anime|documentary|reality`), which already exists — a seed with `subtype='anime'`
  promotes the same subtype, and a non-anime seed demotes it.
- The inverse failure also exists: a hard "same subtype only" filter would wall anime off from
  animation generally. Keep it a soft promotion; the tiers already give the ordering a place to put
  it.

### 5.6 Region and language bias

- `popularity` is computed from a globally skewed user base (§2.8), so using it to rank is a bias
  decision even when it does not look like one.
- `sort_by=vote_average.desc` **without** `vote_count.gte` is a wall of `10.0`-from-one-vote — the
  repository already fixes this with `RATING_VOTE_FLOOR`; **do not remove that floor** and do not
  let a new discover call bypass it.
- `with_original_language` is a hard filter; using it to "keep the rail relevant" is the mechanism
  by which a media app becomes English-only. Use the soft `language` signal of §2.6.
- `region` changes which release date is compared, per the discover docs' "Advanced Filtering"
  section. If a rail ever passes `region`, it changes the *semantics* of `primary_release_date`, not
  just the filtering — read that section before adding it.
- The catalogue's `origin_country` is a JSON array in one `TEXT` column. It is fine as a display
  field and unsuitable as a filter without a join table; do not build a region filter on a `LIKE`.

### 5.7 Small mechanical traps

- **Keyword ids are ids.** `with_keywords` takes keyword **ids**, not names. Names come from
  `/search/keyword` (a network call) or from `title_keyword` (already local). Never invent an id.
- **The two keywords endpoints disagree on the key** (`keywords` vs `results`, §1.4).
- **Collection parts are named `name`, not `title`** (§1.2).
- **`append_to_response` is capped at 20 tokens** and a slash must be percent-encoded (§1.3).
- **`/similar` and `/recommendations` accept only `language` and `page`.** There is no `with_genres`
  on either; a caller who wants filters needs `/discover`.
- **A `page` past 500 fails rather than clamping** (§1.3) — do not page these endpoints at all.
- **`contentless_delete=1` tombstones** slightly skew FTS5's IDF until a merge (§2.3).
- **`columnsize=0` would silently break `bm25()`** on the contentless FTS table (§2.3).
- **Two titles can share a normalized name and be different films, and the visible `title` has no
  `UNIQUE` on `(name, release_date)`.** Dedupe on `external_id`, never on the name.
- **The 180-day TMDB cache ceiling applies to collection rows** (§1.6) — the sweep must cover the new
  table, not only `title`.

---

## 6. What already exists in this working tree

This matters more than any recommendation above, because part of it is already done and the rest has
to slot in beside it. As of this research, `git status` shows **uncommitted work in
`crates/peregrine-providers/src/tmdb.rs` and `crates/peregrine-app/src/runtime.rs`** (71 files
modified overall) containing:

- `recommendations_url` → `/movie|tv/{id}/recommendations`, with `similar_url` kept as the documented
  fallback, and a doc comment citing TMDB staff. **S0's endpoint half is done.**
- `movie_url` + `collection_url` + `collection_of` + `collection_parts`, keyed off
  `belongs_to_collection`, with an explicit "only a film can be in a collection, so a series pays no
  request for one" comment. **S0's franchise half is done**, including release ordering and seed
  exclusion.
- A test, `a_film_s_franchise_leads_the_recommendations_in_release_order`, that is exactly the
  reported bug as a fixture.
- `Runtime::similar` still returns the provider's list **as-is**: no local ranking, no tiers, no
  reasons. **S1–S8 are not started.**
- `RATING_VOTE_FLOOR = 200` exists and is applied whenever a browse sorts by score. **S7's floor
  precedent is done**; the rail's own Bayesian tiebreak is not.
- There is **no `collection` table** in `docs/DATA_MODEL.md` — `collection_of`/`collection_parts`
  answer live from the provider every time. **S1 is the missing piece** and is what makes the
  franchise tier offline and the reason string possible.
- `title_genre` and `title_keyword` have `PRIMARY KEY (title_id, genre_id)` /
  `(title_id, keyword_id)` and **no reverse index**, which is the index the local candidate query
  needs. **S1 must add them.**

Two corrections worth making while this is fresh:

1. `Tmdb::rail`'s comment says "TMDB documents 500 pages". The official reference does **not**
   document a page cap; a TMDB moderator states 500 on the support forum (§1.3). The comment should
   say so, or the claim should be dropped.
2. The collection-parts test fixture uses `{"id": …, "title": …}`. The documented shape uses `name`.
   `SearchResult` reads both, so the code is right — but the `name` spelling is the one TMDB's
   documentation actually shows and it is untested.

---

## 7. Open questions and explicitly unverified claims

| Claim | Status |
| --- | --- |
| TMDB page cap is 500 | Staff-forum statement, contradicted by an old wrapper's "1000". Not in the official reference. Moot for this design (page 1 only). |
| Requests past the cap error rather than clamp | Third-party summary only. Should be checked once with a real key before being written in a doc comment. |
| `X-RateLimit-*` headers exist | Third-party summary only. Do not depend on them; log if seen. |
| The exact latency of a TMDB call | **Not measured.** No number is claimed anywhere in this document. |
| A live TV details response never contains `belongs_to_collection` | Established from the documented example and the absence of any TV collection endpoint. Not confirmed against a live response (no API key in this session). A one-line runtime assertion is worth adding. |
| Whether `/recommendations` ever includes the seed | Not reported either way. Filter unconditionally. |
| The Avatar collection id (`87096`) | Taken from an existing fixture in this repository, not verified against the live API in this session. |
| The local candidate query's cost at 20 000 titles | **Not measured.** The index gap in §6 is the likely reason it would be slow; measure after S1. |
| The starting weights in §2.9 | Deliberately a starting point, not a result. They must earn their place on the golden fixture of §2.10 item 2 before anyone calls them tuned. |
| Whether FTS5 BM25 or TF-IDF cosine orders the golden set better | Unknown; that is what the golden fixture is for. BM25 is recommended first only because the index already exists. |
