# Peregrine — External Data Sources & Wire Protocols

Every fact below was verified live on **2026-10-08** by fetching docs pages and probing endpoints (HTTP status + response bytes captured). Anything not confirmable is marked `unverified`; nothing is inferred.

## Summary table

| Source | What it gives us | Auth | Rate limit | Cache semantics | License/ToS | Verdict |
|---|---|---|---|---|---|---|
| TMDB v3 | Trending, discover (30+ filters), movie/tv/season/episode details, credits, videos, images, external_ids, release_dates, changes | Bearer token in `Authorization` header, or `?api_key=` | Legacy 40/10s disabled 2019-12-16; "somewhere in the 40 requests per second range"; respect 429 | No documented ETag/Last-Modified on the JSON API | Free non-commercial; attribution + logo required; **cache max 6 months**; no commercial use | **primary** |
| TMDB v4 | Same data under `/4/`, plus user lists/auth | Bearer user token | as v3 | as v3 | as v3 | optional |
| TVDB v4 | Series/movie/episode, **absolute/dvd/official season orders**, `/updates` incremental | `POST /login {apikey}` → JWT (~1 month) | not documented on any page verified — unverified | none documented | Free <$50k revenue **requires attribution**; PIN optional | optional (user key) |
| OMDb | Plot, IMDb/RT/Metacritic ratings, poster API | `?apikey=` | Free tier **1,000/day** | none documented | **CC BY-NC 4.0** | optional |
| IMDb datasets | Ratings, alt titles, episode lists, crew | none | none published | Daily refresh ~07:00–08:00 UTC; S3 `Last-Modified` + `ETag` present | "personal and non-commercial use"; local copies permitted | **primary** (ratings only) |
| Trakt | Watch progress, calendars, scrobble, recommendations | Device OAuth → bearer; access token 7d + refresh | AUTHED POST 1/s; AUTHED GET 1000/5min; UNAUTHED GET 1000/5min | `X-Ratelimit`, `Retry-After` | unverified | **primary** (progress) |
| Simkl | Watch progress, anime (AniDB-based), `/search/file` filename matching | `?client_id=` or `simkl-api-key` header; PIN flow for desktop | 10 GET/s, 1 POST/s + per-user daily (V2) or per-app daily (V1) | `X-RateLimit-Limit/Remaining` (V2 only) | unverified | optional |
| AniList | Anime metadata + ordering | GraphQL, optional OAuth | 90/min, **currently degraded to 30/min (live `x-ratelimit-limit: 30`)**; burst limiter | `X-RateLimit-*`, `Retry-After`, `X-RateLimit-Reset` | unverified | optional (anime) |
| MyAnimeList v2 | Anime metadata, list sync | OAuth2 implicit + `X-MAL-CLIENT-ID` for public reads | unverified | `fields`, `limit` (max 100), `offset` | unverified | optional |
| TMDB image CDN | Posters/backdrops/stills | none | none | **No ETag**; `cache-control: public, max-age=31919000` (~369d); `If-Modified-Since` returned **200, not 304** | via TMDB ToS; "no image hosting for ads" | **primary** |
| TVDB artwork | Posters/banners | none | none | unverified | via TVDB terms | optional |
| YouTube Data API v3 | Trailer metadata | OAuth/API key | `search.list` **own bucket, 100 calls/day**, cost 1; all other endpoints share **10,000 units/day** | none | embedding allowed, downloading prohibited | rejected |
| YouTube oEmbed | Verified trailer title/thumbnail, keyless | none | none published | `thumbnail_url` → `i.ytimg.com` | YouTube ToS | optional |
| yt-dlp | Offline trailers | none | n/a | local files | Unlicense; PyInstaller bundles are **GPLv3+** | opt-in only |
| Apple trailers RSS | — | — | — | — | — | **rejected (dead)** |
| Dailymotion API | Alternate trailer hosting | `client_id` for most | unverified | none | unverified | optional |
| archive.org | Public-domain trailers | none | none published | none | per-item | optional |
| BitTorrent/DHT/PEX/LSD | Swarm discovery, metadata | none | n/a | n/a | BEP specs are open | **primary** |
| Prowlarr local API | Aggregated indexer search | `X-Api-Key` header or `?apikey=` | none | none | GPL-3.0 | **primary** (user-supplied) |
| Jackett local API | Aggregated indexer search | `?apikey=` | none | none | GPL-2.0 | optional |
| EZTV API | TV torrents | none | unverified (301s under load) | none | legal risk | disabled by default |
| apibay (TPB) | Torrent search | none | observed **429** on detail endpoint | none | legal risk | disabled by default |
| Nyaa RSS | Anime torrents | none | unverified | unverified | legal risk | disabled by default |
| YTS API | Movie torrents | none | unverified | none | legal risk; domain unstable (`yts.mx` DNS failed twice) | disabled by default |
| OpenSubtitles REST | Subtitles | `Api-Key` header + mandatory `User-Agent`; optional `/login` | quota per tier (unverified) | none | attribution required (free tier, unverified) | **primary** |
| Subdl | Subtitles | `api_key` query | 403 without key | none | unverified | optional |
| Podnapisi | Subtitles | — | — | — | — | **rejected (DNS unreachable)** |
| Chromecast CASTV2 | Casting | none (LAN) | n/a | n/a | Google Cast ToS | **primary** |
| DLNA/UPnP AV | Casting to TVs/Sonos | none (LAN) | n/a | n/a | open specs | **primary** |
| mpv (JSON IPC) | Player integration | local socket | n/a | `--cache`, `--cache-on-disk` | GPL-2.0+/LGPL | **primary** |
| ffmpeg/ffprobe | Probing, subtitle extraction | none | n/a | n/a | LGPL/GPL by build | **primary** |

## Disk budget model

### IMDb dataset sizes — measured, not estimated

`curl -sSI https://datasets.imdbws.com/<file>` gives real compressed sizes. Every uncompressed figure marked *measured* was produced by piping `gunzip -c | wc -lc`; nothing here is extrapolated unless labeled.

| File | gz bytes | gz MiB | Uncompressed | Rows | Last-Modified |
|---|---|---|---|---|---|
| `title.ratings.tsv.gz` | 8,680,932 | 8.3 | **30,019,334** (28.6 MiB) measured | 1,718,390 | 2026-10-08 |
| `title.episode.tsv.gz` | 54,934,731 | 52.4 | **261,198,944** (249.1 MiB) measured | 9,934,901 | 2026-10-08 |
| `title.crew.tsv.gz` | 83,461,236 | 79.6 | **422,831,355** (403.2 MiB) measured | 12,846,867 | 2026-10-08 |
| `title.basics.tsv.gz` | 228,236,026 | 217.7 | **1,115,494,552** (1.04 GiB) measured | 12,846,867 | 2026-10-08 |
| `title.akas.tsv.gz` | 516,760,953 | 492.8 | unverified | — | 2026-10-07 |
| `name.basics.tsv.gz` | 310,996,323 | 296.6 | unverified | — | 2026-10-08 |
| `title.principals.tsv.gz` | 785,835,238 | 749.4 | unverified | — | 2026-10-08 |
| **Total (7)** | **1,988,905,439** (1.85 GiB) | | | | |

The exact seven-file list is confirmed from the bucket index: `name.basics`, `title.akas`, `title.basics`, `title.crew`, `title.episode`, `title.principals`, `title.ratings`. Measured compression spans **3.46x (ratings) to 5.07x (crew)**; applying that band to the three unmeasured files gives an **estimated 6.4–8.2 GB** full-set uncompressed footprint — an estimate, not a measurement.

**Is `title.ratings.tsv.gz` alone sufficient for rating sync? Yes.** It is a self-contained `tconst → averageRating, numVotes` table: 8.68 MB compressed / 30.0 MB raw / 1.72 M rows, refreshed daily, no join required. In SQLite (`tconst TEXT PRIMARY KEY, rating REAL, votes INTEGER`) expect ~45 MB.

**IMDb datasets are not needed for id mapping from TMDB ids.** `GET /3/movie/{id}/external_ids` returns `imdb_id` directly, and `GET /3/find/{external_id}?external_source=imdb_id` reverses it, so `title.basics` (1.04 GiB raw) is unnecessary for mapping. The datasets are keyed on `tconst` and contain **no TMDB ids** — there is no crosswalk inside them, and `title.akas` carries no TMDB ids either, so alt-title data must be re-keyed via TMDB's `imdb_id`. Conditional re-download is possible: `ETag: "a3e42b546ae0d59ed85259c02fdfc8c6-2"` and `x-amz-meta-run-date: 2026-10-07` were both present on `title.ratings.tsv.gz`.

### Recommended default caps

Poster unit costs from real `Content-Length` (single sample): `w92` 6,745 B · `w185` 17,824 B · `w342` 63,663 B · `w500` 103,520 B · `w780` 291,246 B · `original` 1,192,832 B.

| Bucket | Default cap | Hard cap | Notes |
|---|---|---|---|
| SQLite metadata (titles, episodes, external ids) | 400 MB | 1.5 GB | |
| `title.ratings` (gz kept + SQLite) | 60 MB | 60 MB | 8.7 MB gz + ~45 MB table; delete the raw TSV after ingest |
| Episode index (only titles in the user's library) | 120 MB | 400 MB | never ingest all 9.93 M rows |
| `title.akas` | **disabled** | 2.5 GB | only if the user opts into alt-title search |
| `title.principals` | **disabled** | — | never needed; cast comes from TMDB credits |
| `name.basics` | **disabled** | — | |
| Poster/backdrop LRU (content-addressed) | 512 MB | 2 GB | serve `w342`/`w780`; evict by LRU, not TTL |
| LQIP blurhash in DB | ~40 B/row | — | 5,000 titles ≈ 200 KB |
| yt-dlp trailer cache (opt-in) | 0 (off) | 5 GB | user-managed directory |
| **Total default footprint** | **~1.2 GB** | **4 GB** | |

Freshness vs. cost: TMDB ToS caps cached *data* at 6 months, so schedule a metadata refresh sweep every ≤180 days. Images are served with `max-age=31919000` (~369 days) and cannot be revalidated (no ETag, `If-Modified-Since` ignored), so an image may be retained indefinitely while its `file_path` is unchanged — TMDB allocates a new `file_path` per uploaded asset, making each path effectively immutable.

---

## 1. Metadata

### 1.1 TMDB v3

Base `https://api.themoviedb.org/3`. Auth: `Authorization: Bearer <token>` — every reference page's OpenAPI declares `apiKey` in header `Authorization` with `x-bearer-format: bearer`. No key → `401 {"status_code":7,...}` (verified).

- Trending `GET /3/trending/{media_type}/{time_window}` (`all|movie|tv|person`, `day|week`) — listed in the reference index; param schema **unverified** (fetch was rate-limited).
- Discover movies `GET /3/discover/movie`. Full verified parameter set: `certification`, `certification.gte`, `certification.lte`, `certification_country`, `include_adult`, `include_video`, `language`, `page`, `primary_release_year`, `primary_release_date.gte`, `primary_release_date.lte`, `region`, `release_date.gte`, `release_date.lte`, `sort_by`, `vote_average.gte`, `vote_average.lte`, `vote_count.gte`, `vote_count.lte`, `watch_region`, `with_cast`, `with_companies`, `with_crew`, `with_genres`, `with_keywords`, `with_origin_country`, `with_original_language`, `with_people`, `with_release_type`, `with_runtime.gte`, `with_runtime.lte`, `with_watch_monetization_types`, `with_watch_providers`, `without_companies`, `without_genres`, `without_keywords`, `without_watch_providers`, `year`.
- `sort_by` enum (verified): `original_title.asc|desc`, `popularity.asc|desc`, `revenue.asc|desc`, `primary_release_date.asc|desc`, `title.asc|desc`, `vote_average.asc|desc`, `vote_count.asc|desc`. Default `popularity.desc`. `with_release_type` numeric values **unverified**.
- Discover TV `GET /3/discover/tv`. Movie details `GET /3/movie/{movie_id}` with `append_to_response`, `language`. TV `GET /3/tv/{series_id}`; season `/3/tv/{id}/season/{n}`; episode `/3/tv/{id}/season/{n}/episode/{n}`.
- Credits: `/3/movie/{id}/credits`; TV `/3/tv/{id}/aggregate_credits`.
- Videos: `GET /3/movie/{id}/videos?language=en-US` → `{id, results[]}` with `iso_639_1`, `iso_3166_1`, `name`, `key`, `site` (`"YouTube"`), `size`, `type` (`"Trailer"`), `official`, `published_at`. Sufficient for trailer links without the YouTube Data API.
- Images: `/3/movie/{id}/images`, `/3/tv/{id}/images`, `/3/tv/{id}/season/{n}/images`, `/3/tv/{id}/season/{n}/episode/{n}/images`.
- External IDs `GET /3/movie/{id}/external_ids` → `{id, imdb_id, wikidata_id, facebook_id, instagram_id, twitter_id}` (verified). TV equivalent adds `tvdb_id`.
- Age ratings: `GET /3/movie/{id}/release_dates` → `{id, results[]}` with `results[].release_dates[].certification` per `iso_3166_1`. TV `GET /3/tv/{id}/content_ratings` — **unverified** (429).
- Reverse lookup `GET /3/find/{external_id}?external_source=<enum>`. Verified enum: `imdb_id`, `facebook_id`, `instagram_id`, `tvdb_id`, `tiktok_id`, `twitter_id`, `wikidata_id`, `youtube_id`. Response keys `movie_results`, `person_results`, `tv_results`, `tv_episode_results`, `tv_season_results`.
- `append_to_response` collapses e.g. `?append_to_response=videos,credits,images,external_ids` into one request. Use it aggressively — cheapest way to cut request count.

**Changes API (incremental sync), two layers.** (1) `GET /3/movie/changes` — "a list of all of the movie ids that have been changed in the past 24 hours"; up to 14 days via `start_date`/`end_date`; **100 items per page**; params `end_date`, `page`, `start_date`. Same for `/3/tv/changes` and `/3/person/changes`. (2) `GET /3/movie/{id}/changes` — per-entity detail, "by default only the last 24 hours are returned", params `end_date`, `page`, `start_date`; response `changes[]` with `key` (e.g. `images`, `plot_keywords`) and `items[]` carrying `id`, `action` (`added`), `time` (`"2023-04-08 16:35:05 UTC"`), `iso_639_1`, `iso_3166_1`, `value`. Valid `key` values come from `/3/configuration`'s `change_keys` array (verified, 50+ entries including `images`, `imdb_id`, `runtime`, `videos`, `releases`, `certifications`, `episode`).

**Rate limits.** The legacy `40 requests every 10 seconds` was **disabled 2019-12-16**. Current documented limit: "somewhere in the 40 requests per second range… This limit could change at any time so be respectful of the service we have built and respect the `429` if you receive one." Enforcement is live: a burst of doc fetches during this research drew a genuine `429` from `developer.themoviedb.org`.

**Attribution (verbatim).** "You shall use the TMDB logo to identify your use of the TMDB APIs." Required notice: `This [website, program, service, application, product] uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.` The FAQ words it as "This product uses the TMDB API but is not endorsed or certified by TMDB."

**Caching images locally (ToS).** The restrictions list explicitly prohibits using TMDB "as an image hosting service for banner advertisements, graphics, etc." and to "Cache, for longer than 6 months, any information obtained through or from TMDB or the TMDB APIs". On termination "you must promptly delete or otherwise purge all TMDB Content, including any cached content." Commercial use requires a separate written agreement; ML training on TMDB content is prohibited.

**Daily ID exports** at `https://files.tmdb.org/p/exports/`: `movie_ids_MM_DD_YYYY.json.gz`, `tv_series_ids_…`, `person_ids_…`, `collection_ids_…`, `tv_network_ids_…`, `keyword_ids_…`, `production_company_ids_…`. No auth. Job runs from ~07:00 UTC, complete by 08:00 UTC. One JSON object per line, **not** a JSON array. **Files are deleted after 3 months.** Cheapest way to build a local "does this ID exist / how popular is it" index without touching the API.

### 1.2 TMDB v4

Same data under `/4/`, used for user-scoped operations and the newer auth model. `/3/configuration` documents `servers: [{url: "https://api.themoviedb.org"}]` with the version as a path segment. Everything Peregrine needs for catalogue reads is on v3; treat v4 as optional.

### 1.3 TVDB v4 (optional user-supplied key)

OpenAPI spec `https://thetvdb.github.io/v4-api/swagger.yml` (verified 200, 115,347 bytes). `servers: [{url: 'https://api4.thetvdb.com/v4'}]`; `securitySchemes.bearerAuth` = HTTP bearer JWT.

- Auth `POST /login` with `{"apikey": "...", "pin": "..."}` (`pin` optional) → `data.token`, "one month validation length". Live with an empty key: `401 {"status":"failure","message":"InvalidAPIKey: apikey invalid"}`. Unauthenticated `GET /series/121361/extended` → `401 {"message":"Unauthorized"}`.
- Incremental sync `GET /updates?since=<epoch>&type=<entity>&action=delete|update&page=<n>`. `methodInt` = created (1) / updated (2) / deleted (3); deletions may carry `mergeToType` + `mergeToId`. `type` enum includes `artwork`, `episodes`, `movies`, `people`, `seasons`, `series`, `translatedepisodes`, `content_ratings`.
- **Absolute numbering:** `GET /series/{id}/episodes/{season-type}` where `season-type` ∈ `official | dvd | absolute`; plus `/series/{id}/episodes/{season-type}/{lang}`. This is the anime absolute-order mechanism. ID mapping `GET /search/remoteid/{remoteId}` — "Search a series, movie, people, episode, company or season by specific remote id… Allows searching for an IMDB or EIDR id, for example." Also `/series/{id}/nextAired`, `/movies`, `/seasons`, `/search` (index limited to 5k results; `limit` param). **Rate limits: not documented on any page verified** (`api-information` and the repo `README.md` carry no numeric limit) — unverified.
- **Licence:** company revenue **< $50k/year = free, requires attribution**; $50k–$250k = $1,000/yr; $250k–$1M = $10,000/yr. Sample attribution: "Metadata provided by TheTVDB. Please consider adding missing information or subscribing." "Unless approved by TheTVDB, attribution with a direct link to TheTVDB.com must be displayed to end users viewing metadata from our API." User subscription **$11.99/year** (PIN "not required for most projects"). The README recommends one of: a full DB copy monitored via `/updates`, a caching proxy (e.g. squid with per-endpoint TTLs), or direct access with a **subscriber-supported key requiring each user to hold a $12/year subscription**. It also lists endpoints safe to cache "for a week or even longer": artwork types, award categories, content ratings, entity types, genders, genres, inspiration types, languages, movie statuses, people types, series statuses, source types.

### 1.4 OMDb

`https://www.omdbapi.com/?apikey=[yourkey]&`. Params `i` (IMDb ID), `t` (title), `type` (`movie|series|episode`), `y`, `plot` (`short|full`), `r` (`json|xml`), `callback`, `v`. Search `s` + `page` (**1–100**). Season/episode via `Season`, `Episode`. Poster API is patron-only at `http://img.omdbapi.com/?apikey=[yourkey]&`. No key → `{"Response":"False","Error":"No API key provided."}` (verified). **Free tier = 1,000 daily limit**, verified on the key page. **Licence: "All content licensed under CC BY-NC 4.0"**, and "This site is not endorsed by or affiliated with IMDb.com". NonCommercial makes it unusable as a primary catalogue source.

### 1.5 IMDb official datasets

Location `https://datasets.imdbws.com/`. Verified terms: *"Subsets of IMDb data are available for access to customers for personal and non-commercial use. You can hold local copies of this data, and it is subject to our terms and conditions. Please refer to the Non-Commercial Licensing and copyright/license and verify compliance."* Refreshed **daily**. Gzipped TSV, UTF-8, first line is a header, `\N` marks null. Notice on the page: as of 2024-03-18 the datasets are backed by a new data source with "no change in location or schema".

Fields worth knowing — `title.basics`: `tconst, titleType, primaryTitle, originalTitle, isAdult, startYear, endYear, runtimeMinutes, genres`, where `titleType` includes `movie, short, tvseries, tvepisode, video`; `title.ratings`: `tconst, averageRating, numVotes` (sample verified `tt0000001 5.7 2231`); `title.akas`: `titleId, ordering, title, region, language, types[], attributes[], isOriginalTitle`, whose `types` enum includes `alternative, dvd, festival, tv, video, working, original, imdbDisplay`; `title.episode`: episode→series linkage; `title.crew`: `tconst → directors, writers`; plus `name.basics` and `title.principals` for people and cast/crew roles.

Mapped to the four asks: **(a) ratings sync → `title.ratings.tsv.gz` alone, sufficient, 8.68 MB**; **(b) id mapping from TMDB ids → not needed, use TMDB `/external_ids`**; **(c) alternative titles → `title.akas.tsv.gz` (517 MB gz)**; **(d) episode lists → `title.episode.tsv.gz` (54.9 MB gz / 249 MB raw) joined to `title.basics` for titles**.

### 1.6 Watch-progress and anime sources

**Trakt.** Current docs `https://docs.trakt.tv`; the legacy blueprint `https://jsapi.apiary.io/apis/trakt.apib` (854,683 bytes) carries an explicit deprecation banner pointing there but still documents the protocol.
- Headers: `Content-Type: application/json`, `trakt-api-version: 2`, `trakt-api-key: <client_id>`; user calls add `Authorization: Bearer <access_token>`.
- **Device OAuth:** `POST /oauth/device/code` with `{"client_id": "..."}` → `{device_code, user_code, verification_url, expires_in: 600, interval: 5}`. Poll `POST /oauth/device/token` with `device_code` at `interval`, stop at `expires_in`. `access_token` valid **7 days**; persist `refresh_token`. QR helper `https://trakt.tv/activate/{user_code}`.
- **Rate limits:** `AUTHED_API_POST_LIMIT` (POST/PUT/DELETE) **1 call/s**; `AUTHED_API_GET_LIMIT` (GET) **1000/5 min**; `UNAUTHED_API_GET_LIMIT` (GET) **1000/5 min**. Headers `X-Ratelimit` (JSON: `name`, `period`, `limit`, `remaining`, `until`) and `Retry-After`.
- Playback sync `POST /scrobble/start|pause|stop`: progress between 1% and 79% is treated as a *pause* and resumable via `/sync/playback`. `GET /sync/last_activities` is the cheap gate before any pull; also `/sync/watched/{type}`, `/sync/history/{type}/{id}`, `/sync/collection/{type}`, `/sync/ratings/{type}/{rating}`, `/sync/playback`.
- Calendars: `GET /calendars/all/shows/{start_date}/{days}`, `/calendars/my/shows/{start_date}/{days}`, plus `.../shows/new`, `.../shows/premieres`, `.../shows/finales`, `/calendars/all/movies/{start_date}/{days}`. Unauthenticated `GET /calendars/all/shows/2026-10-08/7` → **403**. Cleanest free "new episode tonight" feed. Trakt ToS **unverified**.

**Simkl.** Docs `https://api.simkl.org` with `llms.txt` and OpenAPI 3.1 at `https://api.simkl.org/openapi.json` (1.19 MB, 55 paths). `servers`: `https://api.simkl.com`, plus `https://data.simkl.in` as a no-auth CDN for Calendar + Trending files. Auth is `?client_id=` **or** the `simkl-api-key` header (pick one; query preferred); user calls use `Authorization: Bearer <token>`; invalid client id → `412 {"error":"client_id_failed"}` (verified live).
- **PIN flow (desktop-friendly):** `GET /oauth/pin` → 5-char `user_code`, `verification_uri`, `expires_in` **15 min**, `interval` **5 s**; no `client_secret`. This is AUTH V1; V2 mandates PKCE S256 and V1 retires ~April 2027.
- **Rate limits:** **10 GET/s, 1 POST/s** concurrently with a daily quota. Three distinct 429 bodies: `rate_limit` (per-second, clears ~1 s, *ignore* `Retry-After`), `user_limit_exceeded` (V2 per-user daily), `app_limit_exceeded` (V1 app-wide daily). `X-RateLimit-Limit`/`Remaining` are sent for **AUTH V2 bearer tokens only** (from the second request onward). Exempt from both limits: detail lookups (`/movies/{id}`, `/tv/{id}`, `/anime/{id}`, episode lists), `/redirect`, and the trending/calendar CDN files.
- Incremental sync: `GET /sync/activities` ("Always call this first"), `GET /changes?date_from=&type=anime,shows,movies`, `/sync/all-items/{type}/{status}`, `/sync/history`, `/sync/playback`, `POST /scrobble/start|pause|stop|checkin`. `POST /search/file` identifies a video file the user just opened — "Pass one filename and Simkl returns the matched movie, or the show + the specific episode the filename names. Built for desktop scrobblers and player overlays" (doc warns it is "Not for library scraping").
- **Anime:** Simkl "uses AniDB as the primary source for anime data, which has different season/episode numbering than TMDB/TVDB"; scrobble responses return both Simkl/AniDB `season`/`number` **and** the original `tvdb_season`/`tvdb_number`. Simkl ToS **unverified**.

**AniList** (`https://graphql.anilist.co`, docs `https://docs.anilist.co`). Documented limit 90 req/min, **currently degraded to 30/min** ("temporary measure until the API is fully restored"), plus a burst limiter and a 1-minute timeout on breach with `Retry-After` + `X-RateLimit-Reset` and a GraphQL `errors[].status: 429`. Live probe returned `x-ratelimit-limit: 30`, `x-ratelimit-remaining: 29`, confirming the degraded figure is what you actually get. Rate-limit raises are **not** being accepted. Usable per-title on demand; not a library-wide ordering authority. ToS unverified.

**MyAnimeList v2** (`https://api.myanimelist.net/v2`, docs `https://myanimelist.net/apiconfig/references/api/v2`). OAuth2 implicit, authorize URL `https://myanimelist.net/v1/oauth2/authorize`, scope `write:users`; login-free reads use header `X-MAL-CLIENT-ID`. Live: no client id → `403 {"error":"forbidden"}`; bogus → `400 {"error":"bad_request","message":"Invalid client id"}`. List params `limit` (default 100, **max 100**), `offset`, `fields` (nested selection, e.g. `fields=synopsis,my_list_status{priority,comments}`), `nsfw`; paging via `paging.previous`/`next`; errors `{error, message}`. Rate limit **unverified**.

### 1.7 Anime episode ordering

| Authority | Model | Mechanism |
|---|---|---|
| **TVDB** | Multiple parallel season orders per series, one flagged default ("Aired Order") | `GET /series/{id}/episodes/absolute` vs `.../official` vs `.../dvd`. Base record carries the default season-order id; seasons can be *named* (README cites Money Heist, whose Netflix recut is a different season type). |
| **AniDB** (via Simkl) | Single canonical season per series | Simkl: "anime is treated as a single canonical season per AniDB"; specials use `type: "special"` and lack a `season`/`episode` pair. Simkl maps TMDB/TVDB scrobbles onto AniDB numbers and returns both. |
| **TMDB** | Aired/seasons model; alternative groupings via episode groups | The episode-groups doc page (`reference/tv-episode-group-details`) exists (verified 200), but schema/params are **unverified** (429). |

Store **all three** numbering vectors per episode row (`tmdb_sxxeyy`, `tvdb_absolute`, `anidb_absolute`) and never derive one from another. Simkl's scrobble response is the only free source found that returns the mapping in-band.

### 1.8 Content ratings (age ratings)

TMDB movies: `GET /3/movie/{id}/release_dates` → `results[].release_dates[].certification` keyed by `iso_3166_1`. TMDB TV: `/3/tv/{id}/content_ratings` — endpoint name confirmed in the reference index, response shape **unverified** (429). Free fallback: TVDB's `/content/ratings` is an enumerated lookup, `content_ratings` appears in the `/updates` `type` enum, and the TVDB README lists content ratings among endpoints safe to cache "for a week or even longer". The per-title rating field on TVDB entities is **unverified**.

### 1.9 "Realtime": what is actually achievable

No free API offers push or sub-minute updates. The honest ceiling, cheapest signal first: **TVDB `GET /updates?since=<epoch>`** is genuinely incremental with no 24 h floor (1 req/page) and is the best incremental mechanism of the three providers; **indexer RSS (Prowlarr/Jackett)** lands in minutes and detects *release* appearance rather than metadata, making it the fastest real signal; **TMDB `/movie/changes` + `/tv/changes`** carry a hard **24 h floor** (1 req/page, 100 IDs/page, 14-day window per query), so you cannot ask for "last 15 minutes" and polling more often than daily buys nothing; **TMDB daily ID exports** arrive by 08:00 UTC as 1 unauthenticated GET and are the best bulk "what's new" (files expire after 3 months, so poll daily); **Trakt `/calendars/all/shows/{date}/{days}`** and **Simkl `/changes?date_from=`, `/tv/airing`, `/anime/airing`** are daily, and Simkl's are free on quota because they are CDN-served — Trakt's calendars are the closest thing to "new episode tonight" (add `.../shows/new` and `.../shows/premieres`); **TMDB `discover` + `sort_by=primary_release_date.desc`** is daily at 1 req/page and pairs with `primary_release_date.gte`/`lte`; and the **IMDb datasets** are daily at 8.7 MB–1.85 GB but carry ratings only, not discovery.

**Polite cadence:** metadata change-sweep **once per 24 h** (`/movie/changes` + `/tv/changes` + TVDB `/updates`); Trakt/Simkl calendars **once per 6 h**; indexer RSS **every 15–30 min**; IMDb ratings **daily at a jittered time**; TMDB exports **once/day**. Never poll TMDB faster than its change windows can change.

---

## 2. Images and trailers

### 2.1 TMDB image CDN — verified behaviour

`base_url` `http://image.tmdb.org/t/p/`, `secure_base_url` `https://image.tmdb.org/t/p/`. URL shape `{secure_base_url}{file_size}{file_path}`. Sizes from `/3/configuration`: `backdrop_sizes` `w300, w780, w1280, original`; `logo_sizes` `w45, w92, w154, w185, w300, w500, original`; `poster_sizes` `w92, w154, w185, w342, w500, w780, original`; `profile_sizes` `w45, w185, h632, original`; `still_sizes` `w92, w185, w300, original`. The server is **BunnyCDN** (`server: BunnyCDN-DE1-863`, `cdn-pullzone: 775336`), **not imgix**. Observed on `w342`: `cache-control: public, max-age=31919000`, `last-modified`, **no `etag`** (present on `w500` on one edge but absent on `w92`/`w185`/`w342`/`w780`/`original` — inconsistent, so do not rely on it), `accept-ranges: bytes`.

Revalidation does not work: `If-None-Match` returned 200 and `If-Modified-Since` returned **200 for both a past and a future timestamp**. Conclusion: **treat every `file_path` as immutable and never revalidate.** `logo_path` always returns `.png` for backwards compatibility while `file_type` reveals the true original — SVG logos must be fetched at `original` because they are not resized.

**No imgix-style query params exist.** Adding `?w=200&q=50`, `?auto=format`, or `?fit=clip&q=80&dpr=2` to a `w342` URL returned **byte-identical** responses (identical SHA-256 prefix `7054162fa484ae43f3283cd2` and identical `content-length`). Size must live in the path segment; do not build a query-param sizing layer.

### 2.2 TVDB artwork

Served from `https://artworks.thetvdb.com/`. Verified: `https://artworks.thetvdb.com/banners/posters/81189-1.jpg` → **200, 423,173 bytes**. A fabricated v4-style path returned **403** from CloudFront, so the exact v4 path grammar is **unverified** — read it from the entity payload rather than constructing it. TVDB notes episode images may be 4:3 or 16:9 and that the best artwork is already inlined on the base record, so the full artwork set is usually unnecessary. Cache headers unverified.

### 2.3 Recommended cache strategy

Confirmed viable by the evidence above: the DB stores per image only `remote_url`, `file_path`, `size_variant`, `width/height`, `aspect_ratio`, `iso_639_1`, `vote_average`, and a **blurhash/pHash LQIP (~40 bytes)** — never the bytes. Bytes are fetched on demand into a **content-addressed** LRU disk cache keyed by `sha256(url)`, with an index mapping `url → blob path + byte size + last_access`; because URLs are immutable and non-revalidatable, eviction is purely **LRU by size budget**, not TTL. Cache only two variants: `w342` posters (63 KB) and `w780` backdrops (291 KB), and proxy the rest.

### 2.4 YouTube

**Quota — materially changed from the widely-cited model.** Verified text: "Projects that enable the YouTube Data API have a default quota allocation of **100 `search.list` calls**, 100 `videos.insert` calls, and **10,000 units per day** combined for all other endpoints." And: "The `search.list` and `videos.insert` methods have their own quota buckets. Each of these methods has a default daily limit of **100 per day**. The quota cost is **1 per call**." So `search.list` no longer costs 100 units from a shared pool — it is a separate hard **100 calls/day** bucket. Resets at midnight Pacific; paging costs quota per page. A search-driven trailer lookup is therefore architecturally unacceptable.

**Is TMDB `videos` enough to avoid the YouTube API entirely? Yes.** It already returns `key`, `site`, `type`, `official`, `published_at` — enough to build a link and an embed, with no YouTube API call.

**Keyless oEmbed fallback** (verified 200): `https://www.youtube.com/oembed?url=<video_url>&format=json` → `{title, author_name, author_url, type, height, width, version, provider_name, provider_url, thumbnail_height, thumbnail_width, thumbnail_url, html}`. `thumbnail_url` resolves to `https://i.ytimg.com/vi/<id>/hqdefault.jpg`. Use it to validate a trailer key, get a real title, and obtain a thumbnail with zero quota. `https://noembed.com/embed?url=...` also returned 200 as a secondary proxy.

**Privacy-enhanced embed:** `https://www.youtube-nocookie.com/embed/<id>` returned **200**. Use this domain. **ToS:** embedding through the official player is the supported path and downloading is not; the operative distinction is access through the service's own player/embed rather than separating content from it. The exact verbatim download-prohibition sentence is **unverified** (not extracted).

### 2.5 yt-dlp and trailer alternatives

Licence facts from the README: yt-dlp is **Unlicense**, but "many of the release files contain code from other projects with different licenses" and "Most notably, the PyInstaller-bundled executables include **GPLv3+** licensed code, and as such the combined work is licensed under GPLv3+." The git repo, sdist and wheel are Unlicense-only. yt-dlp now also needs `yt-dlp-ejs` plus a JS runtime (deno recommended) for full YouTube support. Invoking a user-installed `yt-dlp` as a **separate process** therefore keeps Peregrine's own licensing clean, while bundling the PyInstaller binary pulls GPLv3+ into the distribution — which matches AGENTS.md R25 (copyleft tooling runs as a separate executable).

If offered as opt-in, document: off by default and user-installed; downloading YouTube content conflicts with YouTube's ToS and may conflict with local copyright law; the user makes the request from their own machine and IP; Peregrine neither bundles nor redistributes the tool or any media; output lands in a user-chosen directory outside the app's cache budget. **Other sources.** Apple trailers RSS is **dead** — `https://trailers.apple.com/trailers/home/rss/newtrailers.rss` returns **200 but `content-type: text/html`** with an HTML SPA (`<title>Apple TV</title>`), not RSS/XML; same for `www.apple.com/trailers/home/rss/newtrailers.rss` and `trailers.apple.com/trailers/home/xml/current.xml`, and `https://itunes.apple.com/us/rss/newmovies/xml` returned **400**. Dailymotion is live: `https://api.dailymotion.com/videos?search=trailer&fields=id,title,duration&limit=2` returned valid JSON (`total: 1000`; items with `id`, `title`, `duration`), and `https://www.dailymotion.com/services/oembed?url=...` returned 200 (18 bytes — effectively empty for that probe); its rate limits and ToS are unverified. archive.org works: `https://archive.org/advancedsearch.php?q=<query>&fl[]=identifier&fl[]=title&rows=N&output=json` returned 200 with `numFound: 78146` for `mediatype:movies AND subject:trailer`, and the `identifier` feeds `https://archive.org/download/{identifier}/...`; public-domain status is per-item, so filter on licence metadata. No API key needed for either.

---

## 3. Torrent discovery & streaming protocols

### 3.1 Magnet URI and the BEP map

Magnet links are defined as part of **BEP 9**, which states the extension "makes it possible to support magnet links, a link on a web page only containing enough information to join the swarm (the info-hash)". The `xt=urn:btih` variants are `urn:btih:<40-hex>` (legacy/SHA-1) and `urn:btih:<32-char base32>`. BEP 9's post-history notes "16-May-2016: added peer source parameter to magnets" (`xs=`). A `urn:btmh:1220<64-hex>` multihash form exists for v2/hybrid torrents, but the literal prefix was **not** confirmed in BEP 52's fetched text — treat it as unverified.

| Feature | BEP | Verified detail |
|---|---|---|
| Metadata exchange | **BEP 9** | `ut_metadata` key in the BEP 10 handshake plus `metadata_size`. Metadata in **16 KiB (16384 byte)** blocks indexed from 0, last block may be smaller. Bencoded messages with `msg_type` (`request`/`data`/`reject`) and `piece`; responses MUST carry the same `piece`; unrecognized IDs MUST be ignored. Example: `{'m': {'ut_metadata', 3}, 'metadata_size': 31235}` |
| Extension protocol | **BEP 10** | Extension handshake carries the `m` dictionary plus extra keys; basis for `ut_metadata` and `ut_pex` |
| Peer exchange | **BEP 11** | `ut_pex` message; fields `added.f` and `dropped` verified present in the spec |
| Local Service Discovery | **BEP 14** | Title "Local Service Discovery"; the LAN multicast peer-discovery mechanism |
| Canonical peer priority | **BEP 40** | Status **Draft**. Priority formula over IPs to fix swarm-entry barriers and a connection-slot DDoS vector. Post-history: "29-January-2014 Specify **CRC32-C** rather than SHA1 hashing" |
| v2 torrents / hybrids | **BEP 52** | "The BitTorrent Protocol Specification v2", Draft. v2 wire protocol and torrent format added 14-May-2017; SHA-256 merkle trees over v2 piece layers |
| Web seeds | **BEP 19** | Accepted. HTTP/FTP servers act as a "permanently unchoked seed"; metadata need not change; additive for clients that ignore it |
| DHT | **BEP 5** | KRPC over UDP, bencoded, four queries `ping`/`find_node`/`get_peers`/`announce_peer`. "Buckets that have not been changed in 15 minutes should be 'refreshed'"; routing table should persist between runs |

**DHT bootstrap nodes.** BEP 5 warns: *"Please do not automatically add 'router.bittorrent.com' to torrent files or automatically add this node to clients routing tables."* The `nodes` key format is `[["<host>", <port>], ...]`. The only concrete address in the spec is the illustrative `["127.0.0.1", 6881]` / `your.router.node`. The commonly-used bootstrap hosts are **not enumerated in BEP 5** — treat the real list as unverified and make it user-configurable.

### 3.2 Sequential/streaming download in practice

Streaming prioritization is a **deadline-scheduled rarest-first hybrid**: the readahead window is strict sequential priority-0, everything else stays rarest-first so the swarm stays healthy.
- Derive `bitrate_bps` from `ffprobe` (`size_bytes * 8 / duration_secs`, or the container's real stream bitrate).
- `readahead_bytes = readahead_seconds * bitrate_bps / 8`, with `readahead_seconds` at least `piece_size / measured_throughput` × safety factor. Practical floor 30–60 s of media; ~2 minutes for high-bitrate 4K remuxes.
- **Buffer health** = `buffered_seconds` (contiguous verified-present seconds from the playhead) and `buffer_health_ratio = buffered_seconds / readahead_seconds`. Expose both. If the playhead reaches the end of contiguous data, hold playback rather than let the player read a hole.
- Deadline scheduling: give each piece in the readahead window a deadline from its media offset; when a deadline is at risk, cancel the piece from slow peers and re-request from the fastest. This is what separates streaming from downloading.

mpv-side knobs, all verified in the manual: `--cache`, `--cache-secs=<seconds>` ("How many seconds of audio/video to prefetch if the cache is active. This overrides the `--demuxer-readahead-secs` option"), `--demuxer-max-bytes` ("specify the size of the cache"), `--demuxer-readahead-secs`, `--cache-pause` (implicitly disabled unless cache is on), and `--cache-on-disk=<yes|no>` — "Write packet data to a temporary file, instead of keeping them in memory… The cache file is append-only. Even if the player appears to prune data, the file space freed by it is not reused. The cache file is deleted when playback is closed." `--cache-on-disk` therefore **cannot** serve as a torrent piece store: append-only, never reclaims, deleted on close.

**The three delivery approaches compared.**

| | HTTP byte-range server on loopback | FUSE mount | Pipe to stdin |
|---|---|---|---|
| Seek | Native. A new `Range:` request maps back to piece indices and re-prioritizes. Range requests are the same primitive Chromecast/DLNA need, so one implementation serves local playback *and* casting | Native. Player `pread`s at arbitrary offsets; FUSE maps to pieces. Granularity is the read size | Poor. Pipes are forward-only with a limited seekback buffer; a backward seek restarts the stream |
| Compatibility | Universal — mpv, VLC, ffmpeg, browsers, TVs | Needs FUSE: fine on Linux, **macFUSE** on macOS (kernel extension + user approval), WinFsp on Windows | ffmpeg's pipe seekback differs by demuxer |
| Subtitles | Clean — sidecar served over the same server, or `--sub-file=<url>` | Clean — files appear in a tree, so `--sub-file-paths` works | Awkward — one stream cannot carry sidecars; must mux or pass separately |
| Complexity | Moderate: a range-aware handler plus offset→piece map | High: full filesystem semantics (`fstat`, `read`, `readdir`, error mapping) | Lowest to write, highest to get right |
| Same code path as casting | **Yes** | No | No |

**Recommendation: HTTP byte-range server on loopback.** It is the only option that is universal, avoids a macOS kernel extension, gives correct seek semantics, and is reusable verbatim for Chromecast and DLNA.

**Peeri / torlink.** GitHub's repository API returned **403** for every lookup from this host, so individual repos could not be inspected. Repository *search* did work and returned `baairon/torlink` — **5,854 stars, TypeScript**, "A sleek, zero-setup torrent finder and downloader that lives right in your terminal". That establishes torlink as real and popular, but **how it prioritizes and delivers pieces is unverified**. `Peeri` was not found at all — **unverified**.

### 3.3 Trackers and indexers

**Prowlarr local API** (recommended primary, user-supplied). OpenAPI `https://raw.githubusercontent.com/Prowlarr/Prowlarr/develop/src/Prowlarr.Api.V1/openapi.json` (verified 200, 145,360 bytes, 93 paths).
- `servers: [{url: '{protocol}://{hostpath}', variables: {protocol: {default: 'http', enum: ['http','https']}, hostpath: {default: 'localhost:9696'}}}]`.
- Auth: `X-Api-Key` **header** or `apikey` **query parameter** (both are declared security schemes).
- Search: `GET /api/v1/search` with `query`, `type`, `indexerIds[]` (int32 array), `categories[]` (int32 array), `limit`, `offset`. Also `POST /api/v1/search` and `/api/v1/search/bulk`.
- Result schema `ReleaseResource`: `id, guid, age, ageHours, ageMinutes, size (int64), files, grabs, indexerId, indexer, subGroup, releaseHash, title, …` — `size` and `title` together, which is what the release parser and disk estimator need. Plus `GET /api/v1/indexer`, `/api/v1/indexer/{id}`, `/api/v1/history`, `/api/v1/applications`.
- Query-param enums usable for typed clients: `MovieSearchParam` = `q, imdbId, tmdbId, imdbTitle, imdbYear, traktId, genre, doubanId, year`; `TvSearchParam` = `q, season, ep, imdbId, tvdbId, rId, tvMazeId, traktId, tmdbId, doubanId, genre, year`. `tmdbId` is first-class for both, so Peregrine can search by TMDB id with no title matching.

**Jackett** (`https://github.com/Jackett/Jackett`, verified 200): same Torznab / `?apikey=` model, GPL-2.0. Alternative local aggregator; endpoint shapes unverified in detail.

**Public sites** — status verified live: EZTV `https://eztv.re/api/get-torrents?limit=N` → 200 JSON, `torrents_count: 1090198`, later calls **301**; no formal spec found. apibay (TPB) `https://apibay.org/q.php?q=<q>&cat=<n>` → 200 JSON array with keys `id, name, info_hash, leechers, seeders, size, num_files, username, added, status, category, imdb` (undocumented); its detail endpoint `https://apibay.org/t.php?id=<id>` returned **429 Too Many Requests**. `https://thepiratebay.org/apibay/q.php` returns 200 but `text/html`, **not** the API. Nyaa `https://nyaa.si/?page=rss` → 200 `application/xml` RSS with a `nyaa:` namespace (RSS is the API). YTS `https://{host}/api/v2/list_movies.json?limit=N` → `yts.lt` 200 JSON, `yts.rs` 200 JSON, but **`yts.mx` DNS failed twice** (undocumented).

All of these are marked **disabled by default**. `apibay` returning a real `429` is direct evidence these endpoints rate-limit and must never be polled on a schedule; YTS's response even carries a notice in `status_message` ("Base URL moving to https://…") and its mirror set is unstable at the DNS level.

**Defensible default architecture:** Prowlarr and Jackett are first-class, user-configured, key-authenticated local sources (legitimate open-source software the user runs). Public-site connectors ship **present but disabled**, with no default domain list, requiring the user to enable and supply the URL. Do not bundle, mirror, or proxy indexer content and do not embed a default tracker list — that keeps Peregrine a client of user-chosen infrastructure rather than a distributor.

### 3.4 VPN / proxy safety

- **SOCKS5:** the engine takes a `proxy_url` supporting `socks5h://` — the `h` matters, because it resolves DNS at the proxy and prevents tracker/indexer lookups leaking the real resolver. Verify whether both peer TCP *and* UDP-tracker/DHT traffic are in scope; SOCKS5 UDP-associate is not universally implemented, so DHT must be explicitly disabled or explicitly tunnelled when a proxy is set, and the app should document which the engine actually does. **Interface binding:** bind listening and outbound sockets to a named interface and expose the resolved local IP so the user can confirm.
- **Leak detection:** a manual "Check for leaks" action comparing the tunnel-exit IP against the machine's non-tunnelled public IP via external echo endpoints. The specific endpoint list is **unverified** (see spike #19). **Split-tunnel caveats per OS:** macOS and Windows both allow routing only selected apps or subnets through the tunnel, and binding semantics differ — on macOS, binding to a `utun` interface guarantees only that traffic egresses that interface, not that it is encrypted. The correct invariant is **fail closed**: if the bound interface disappears, the engine stops all peer and tracker I/O rather than falling back to the default route.

### 3.5 Release-name parsing

Canonical tokens, most-reliable first. **Title + year:** `Title (YYYY)` or `Title.YYYY`; the year disambiguates remakes. **Season/episode:** `S01E02`, `s01e02e03`, `1x02`, absolute `- 12`, date-based `2024.05.12`, and **season packs** (`S01`, `Season 1`, `COMPLETE`) — a pack is one release with N× episode size, so the disk estimator must not treat it as a single episode. **Multi-episode:** `S01E01-E03`, `S01E01E02`, `1x01-1x02`.
- **Resolution:** `2160p`/`4K`, `1440p`, `1080p`, `1080i`, `720p`, `576p`, `480p`.
- **Source:** `REMUX`, `BluRay`/`BDRip`/`BRRip`, `WEB-DL`/`WEBDL`/`WEB`, `WEBRip`, `HDTV`, `DVDRip`, `HDRip`, `SDTV`, `PDTV`, `CAM`/`TS`/`TC`/`R5`.
- **Codec:** `x264`/`H.264`/`AVC`, `x265`/`H.265`/`HEVC`, `AV1`, `XviD`, `DivX`, `VP9`, profiles `High@L4.1`, `10bit`.
- **HDR:** `HDR`/`HDR10`, `HDR10+`, `DV`/`DoVi`/`Dolby.Vision`, `SDR`.
- **Audio:** `AAC`, `AC3`/`DD`, `EAC3`/`DDP`/`DD+`, `DTS`, `DTS-HD`, `DTS:X`, `TrueHD`, `Atmos`, `FLAC`, `MP3`, `Opus`, `PCM`/`LPCM`; layouts `2.0`, `5.1`, `7.1`.
- **Release group:** the trailing `-GROUP` token. **Proper/repack:** `PROPER`, `REPACK`, `REAL`, `RERIP`, `v2` — these *supersede* an earlier release and are replacements, not duplicates.
- **Other flags:** `MULTi`, `DUAL`, `iNTERNAL`, `EXTENDED`, `REMASTERED`, `UNCUT`, `Criterion`, `Complete`, `Hybrid`. **Language:** `VOSTFR`, `SUBFRENCH`, `ITA`, `GER`, `MULTI`.

Authority for naming conventions: **TRaSH Guides**, `https://trash-guides.info/` (verified 200) and `https://trash-guides.info/Radarr/radarr-setup-quality-profiles/` (verified 200, 260,073 bytes); the older `https://trash-guides.info/Sonarr/Sonarr-Release-Profile-RegEx/` returned only **450 bytes**, so it moved — find the live regex page from the site navigation rather than hardcoding that path. Design: parse to a **confidence-scored token set**, not one normalized string, keeping raw name and parse together so a mis-parse is debuggable, and never key deduplication on a parse — key it on the infohash.

### 3.6 Subtitles

**OpenSubtitles REST API.** Base `https://api.opensubtitles.com/api/v1`. Live-verified: `GET /infos/languages` → 200 `{"data":[{"language_code":"ab","language_name":"Abkhazian"}, …]}`; `GET /subtitles?query=matrix&languages=en` → **301** (follow it); bad key → `403 {"message":"You cannot consume this service","request_id":"..."}`; missing/undescriptive `User-Agent` → `{"error":"Blocked (kong-user-agent-block)"}`. So a descriptive **User-Agent is mandatory** and the edge is Kong. Auth model is an `Api-Key` header plus optional user login via `POST /login` (the login probe was blocked by the user-agent rule, so its payload shape is **unverified**). Download is a separate endpoint issuing a temporary link with a per-tier quota — **quota numbers and free-tier attribution requirement are unverified**. Search parameter names (`query`, `imdb_id`, `tmdb_id`, `hash`, `season_number`, `episode_number`, `languages`) are consistent with the URLs probed but the full documented set is **unverified** (Stoplight docs are JS-rendered; the OpenAPI JSON was 403).

**`moviehash` algorithm — fully verified** from `http://trac.opensubtitles.org/opensubtitles/wiki/HashSourceCodes?format=txt` (200, 65,194 bytes):
- *"it calculates: size + 64bit chksum of the first and last 64k (even if they overlap because the file is smaller than 128k)"*.
- Concretely: `hash = filesize`, then add the little-endian `uint64` chunks over the first 65536 bytes, then seek to `max(0, filesize - 65536)` and add the `uint64` chunks over the last 65536 bytes.
- **Valid file-size range on opensubtitles.org: `131072 < size < 9000000000` bytes.**
- Output must be **zero-padded to 16 hex chars**: the spec's own example is `'101eae5380a4769' => '0101eae5380a4769'`.
- Derived from Media Player Classic (guliverkli), whose hashing source is **GPL** — relevant if you port that code rather than reimplementing from the description.
- **Test vectors for unit tests:** `breakdance.avi`, 12,909,756 bytes → `8e245d9679d31e12`; `dummy.rar` **unpacked**, 4,295,033,890 bytes → `61f7751fc2a72bfb`.

**Other sources.** Subdl `https://api.subdl.com/api/v1/subtitles?api_key=<k>&film_name=<q>` returned **403 `{"status":false,"statusCode":403,"error":"not_authorized"}`** with an invalid key, confirming the endpoint exists and is key-gated; its docs sites are behind a Cloudflare challenge ("Just a moment…"), so params are **unverified**. Podnapisi: `www.podnapisi.net` **did not resolve** (DNS failure) — **rejected on reachability**.

**The sync problem.** Three independent failure modes: (a) a constant time offset (different release, extra logo, different intro); (b) a **frame-rate mismatch** — 23.976 vs 25 vs 29.97 fps — producing *linear drift* where the offset grows with runtime; (c) PAL/NTSC region differences, which manifest as (b). A constant `--sub-delay` fixes only (a); (b) needs either timeline resampling by the fps ratio or full alignment. Two open implementations addressing this: **`ffsubsync`** (speech-based — extracts the audio track's voice-activity signal and cross-correlates it against the subtitle cue timeline to recover offset *and* rate) and **`alass`** (audio-to-subtitle alignment using chunked cross-correlation, designed for the fps-drift case); `SubtitleEdit` offers a GUI equivalent. **Caveat: all three are unverified from this environment — the GitHub repository API returned 403 for every lookup.** They are candidates for the spike list, not confirmed dependencies. Practical plan: try `moviehash` exact match first (a hash match needs no sync); if the offset is constant, fit a single shift; if the residual grows linearly, fit `offset + rate` from the first and last cue and write a corrected file rather than fighting `--sub-delay` alone.

---

## 4. Casting

### 4.1 Chromecast CASTV2

- **Discovery:** mDNS/DNS-SD service type **`_googlecast._tcp.local`** (verified in Google's Cast discovery doc: "Run the utility and find the `_googlecast._tcp.local` service record. This record will describe your Web Receiver device name and model along with service data."). A live mDNS PTR query from this host returned nothing because no Chromecast exists on this LAN — the service name is documented, the TXT-field set (`id`, `fn`, `md`, `rs`, `ve`, `ca`, `st`, `ic`) is **unverified**. **Channel:** CASTV2 runs over **TLS on port 8009** with length-prefixed protobuf frames; namespaces include connection, heartbeat, receiver and media.
- **Media namespace:** `urn:x-cast:com.google.cast.media`, with a **maximum transport message size of 64 KBytes** ("The transport message maximum size is currently 64 KBytes."). **Commands:** `LOAD`, `PLAY`, `PAUSE`, `SEEK`, `STOP`, `GET_STATUS`, `VOLUME`.
- **`LOAD` fields:** `requestId` (int), `type` (`"LOAD"`), `media` (`MediaInformation`), `autoplay` (bool, optional, **default true**). **`MediaInformation`:** `contentId` (string; "In most cases, this will be the URL to the media"; **max length 1k**), `streamType` (enum `NONE` | `BUFFERED` | `LIVE`), `contentType` (MIME), `metadata` (object), and duration. Metadata structs: `GenericMediaMetadata` has `metadataType` **0** (the only value) with `title`, `subtitle`, `images`; `MovieMediaMetadata` has `metadataType` **1**; `TvShowMediaMetadata` also exists. Sending the wrong `metadataType` for the struct is a protocol error. **`SEEK` fields:** `mediaSessionId`, `requestId`, `type` (`"SEEK"`), `resumeState` (optional enum `PLAYBACK_START` | `PLAYBACK_PAUSE`), `currentTime` (double, seconds).
- **`MediaStatus` — how progress comes back:** `mediaSessionId` (int, "set by the receiver at LOAD"), `media` (`MediaInformation`, only if changed), `playbackRate`, `playerState` (`IDLE` | `PLAYING` | `BUFFERING` | `PAUSED`), `idleReason` (optional; `CANCELLED`, `INTERRUPTED`, `FINISHED`, `ERROR`), `currentTime` (double, seconds), `supportedMediaCommands` (flags).
- **Containers the default receiver plays:** `MP2T`, `MP3`, `MP4`, `OGG`, `WAV`, `WebM` (verified list). `MPEG-DASH`, `SmoothStreaming` and `HLS` (AES-128 / SAMPLE-AES via Widevine L1) exist but require the Web Receiver SDK and **CORS**; "Progressive download without adaptive switching" is the plain-HTTP path. Codecs: H.264 High up to L4.1 (1st/2nd gen) or L4.2 (3rd gen), VP8; newer devices add HEVC/H.265 Main/Main10 up to L5.1 and VP9 Profile 0/2. Audio: FLAC, HE-AAC, LC-AAC, MP3, Opus, Vorbis, WAV (LPCM). **Explicit caution: "HEVC is not supported on Transport Stream containers."** MKV is not in the container list at all, so a **remux to MP4 is the common path** for sources the receiver won't take directly.
- **Range support:** the receiver fetches `contentId` over plain HTTP and `SEEK` requires random access, so the serving endpoint must implement **byte-range requests** and advertise `Accept-Ranges: bytes`. This is the same handler recommended in §3.2 — no extra work.
- **Rust crates (crates.io API, verified):** `rust_cast` 0.21.0, published 2025-12-30, 156,706 downloads — best available for CASTV2 channels/control, though media-namespace completeness still needs a spike. `mdns-sd` 0.21.5, published 2026-10-05, 5,938,775 downloads — use for `_googlecast._tcp` discovery; `libmdns` 0.10.1 (2025-09-06, 1,125,818 downloads) is the alternative. `rust-cast` and `castrs` **do not exist** on crates.io (verified NOT FOUND).

### 4.2 DLNA / UPnP AV

- **Discovery — verified by live capture.** An `M-SEARCH` datagram to **`239.255.255.250:1900`** with `MAN: "ssdp:discover"`, `MX: 2`, `ST: urn:schemas-upnp-org:device:MediaRenderer:1` elicited a real response from a Samsung TV on this LAN:
```
HTTP/1.1 200 OK
CACHE-CONTROL: max-age=1800
LOCATION: http://192.168.1.50:9197/dmr
SERVER: Samsung-Linux/4.1, UPnP/1.0, Samsung_UPnP_SDK/1.0
ST: urn:schemas-upnp-org:device:MediaRenderer:1
USN: uuid:<device-uuid>::urn:schemas-upnp-org:device:MediaRenderer:1
BOOTID.UPNP.ORG: 9
```
Working flow: M-SEARCH → parse `LOCATION` → HTTP `GET` the device description XML → find `serviceType`/`controlURL` for `AVTransport` and `ConnectionManager`. `CACHE-CONTROL: max-age=1800` means SSDP responses are themselves cacheable, so re-discovering more often than every 30 minutes is wasteful. No MediaServer existed on this LAN, so `ContentDirectory:Browse` is untested and the `ssdp:all` / `MediaServer:1` targets are **unverified**.

**Service actions for "cast this file":** `ConnectionManager:GetProtocolInfo` (what the renderer accepts), `AVTransport:SetAVTransportURI` (`InstanceID`, `CurrentURI`, `CurrentURIMetaData`), then `AVTransport:Play` / `Pause` / `Stop` / `Seek`, plus `AVTransport:GetPositionInfo` and `GetTransportInfo` for state and position. `ContentDirectory:Browse` (`ObjectID`, `BrowseFlag`, `StartingIndex`, `RequestedCount`, `SortCriteria`) is only needed for a MediaServer, not for pushing to a MediaRenderer. Exact SOAP envelope and argument casing beyond these names is **unverified** (UPnP spec documents not fetched).

**DIDL-Lite requirement:** `CurrentURIMetaData` must carry an escaped `<DIDL-Lite>` document with `dc:title`, `upnp:class` = `object.item.videoItem`, and a `<res>` whose `protocolInfo` follows `http-get:*:<mime-type>:<DLNA.ORG_* profile string>` matching what `GetProtocolInfo` reported. The exact skeleton is **unverified**, but reading `GetProtocolInfo` and matching it is architectural.

**Codec support:** modern TVs and Sonos vary too widely to assume anything; read `GetProtocolInfo` per device and decide remux/transcode from it. Sonos is audio-first. Specific device matrices are **unverified**.

**Rust crates (verified):** `rupnp` 3.0.0 (2025-04-25, 994,171 downloads) is the best-maintained UPnP device/control crate. `ssdp` 0.7.0 was last published **2018-10-05** (86,487 downloads) — **abandoned, do not use**. `dlna` does not exist on crates.io (verified NOT FOUND).

### 4.3 Local HTTP server safety and position sync

- **Bind policy:** loopback-only (`127.0.0.1`) is safe but **cannot serve a TV or Chromecast** — they fetch from their own network stack, so the server must be LAN-reachable. Bind to the specific LAN interface the casting device is on, never to a public interface. Use an **ephemeral port** chosen by the OS and reported to the UI.
- **One-time token in the URL path:** `/s/<token>/<infohash>/<fileindex>`. Single-session, expires when casting stops, rotated per cast session — it is the only thing that authorises a fetch. An **optional PIN** for the whole server is off by default: a PIN on a LAN-only server is friction with little gain, since the token is the real control.
- **No directory listing.** Only exact registered URLs resolve; everything else 404s. There must be no path that enumerates files — that is also what stops the server becoming a general-purpose file server.
- **Position sync:** Chromecast — poll `GET_STATUS` or subscribe to `MEDIA_STATUS` and read `currentTime` + `playerState`; `mediaSessionId` identifies the session and a `FINISHED`/`INTERRUPTED` `idleReason` signals the end. DLNA — poll `AVTransport:GetPositionInfo` (position + track duration) and `GetTransportInfo` (transport state). In both cases write position back on a debounce (every ~15 s and on state transitions) so a network hiccup cannot corrupt resume state.

### 4.4 AirPlay verdict

**Out of scope for non-Apple targets.** A full-text search of the mpv manual returned **zero** occurrences of `airplay` and **zero** of `raop` — there is no mpv support to piggyback on. No established, complete Rust AirPlay 2 sender exists; the protocol is proprietary, uses FairPlay-derived pairing and encryption, and the open ecosystem around it is reverse-engineered and fragile. Recommended posture: do not implement AirPlay; document it as an OS-level capability (`System Settings → Displays → AirPlay`, or the Apple TV app) rather than a Peregrine feature.
## 5. Captions/language and player-level integration

### 5.1 mpv

All names below were confirmed present in the official manual (`https://mpv.io/manual/master/`, 1,331,135 bytes fetched).

**Options:** `--sub-file=<filename>`, `--sub-files=<file-list>`, `--sub-delay=<sec>`, `--secondary-sub-delay=<sec>`, `--sid=<id>`, `--secondary-sid`, `--sub-auto`, `--sub-scale`, `--sub-pos`, `--sub-visibility`, `--sub-font`, `--sub-codepage`, `--sub-ass-override`, `--sub-font-provider=<auto|none|fontconfig>`, `--slang`, `--alang`, `--audio-file`, `--input-ipc-server=<path>`, `--save-position-on-quit`, `--watch-later-options`, `--resume-playback=<yes|no>`, `--resume-playback-check-mtime=<yes|no>`, plus the cache options listed in §3.2.

**Deprecation state of `--sub-file` — the key gotcha, verified.** It is *not* removed; it is now an **alias for `--sub-files-append`**. The manual's list-options section: *"Options of this type (like `--sub-file`, `--audio-file`, `--glsl-shader`) are aliases for the proper option with `-append` action. For example, `--sub-file` is an alias for `--sub-files-append`. Options of this type can be changed at runtime using the `change-list` command, which takes the suffix (without the `-`) as separate operation parameter. An object settings list can hold up to 100 elements."* So `--sub-files` is canonical, `--sub-file` still works, and at runtime you use `change-list sub-files append <url>` rather than `set sub-file`.

**Runtime commands (verified):** `sub-add <url> [<flags> [<title> [<lang>]]]` — "Load the given subtitle file or stream. By default, it is selected as current subtitle after loading." `flags` ∈ `select` (select immediately, default) | `auto` (don't select) | `cached` (select, use the cache). Also `sub-remove`, `sub-reload`, `set`, `cycle`, `add`, `change-list`. The manual warns `--input-ipc-server` "is also suitable for purposes like remote control (however, the IPC protocol itself is not 'secure' and not intended to be so)" — bind the socket in a user-only directory with restrictive permissions and never expose it on the network.

**Properties** for `get_property`/`observe_property`: `sid`, `sub-delay`, `sub-visibility`, `sub-text`, `sub-start`, `sub-end`, `current-tracks/sub`, `track-list`, `chapter-list`, `path`, `media-title`, `duration`, `time-pos`, `pause`, `volume`, `mute`, `speed`, `fullscreen`, `keep-open`.

**`track-list/N/...` subfields (verified):** `id`, `type`, `src`, `title`, `lang`, `selected`, `main-selection`, `external` ("yes/true if the track is an external file… This is set for separate subtitle files"), `external-filename` ("The filename if the track is from an external file, unavailable otherwise"), `codec`, `codec-desc`, `codec-profile`, `default`, `forced`, `dependent`, `visual-impaired`, `hearing-impaired`, `albumart`, `hls-bitrate`, `program-id`, `program-ids`. `external-filename` is the field a subtitle-manager UI needs to distinguish a fetched sidecar from an embedded track.

**Per-file config and paths (verified):** the config file is `mpv.conf` in the mpv config directory; user-specific is `~/.config/mpv/mpv.conf`; system-wide examples `/etc/mpv` or `/usr/local/etc/mpv`; Windows uses `%APPDATA%/mpv/mpv.conf`. **macOS-specific:** "On macOS the watch later directory is located at `~/.config/mpv/watch_later/` and the cache directory is set to `~/Library/Caches/io.mpv/`. These directories can't be overwritten by environment variables." The general user state dir for `watch_later` is typically `~/.local/state/mpv/`. Other config subdirs: `scripts/`, `script-opts/`, `fonts/`, `input.conf`.

**JSON IPC (verified):** enabled with `--input-ipc-server=/path/to/socket` (unix domain socket, or named pipe on Windows); newline-delimited JSON; events `property-change`, `file-loaded`, `end-file`. Launch with `--no-config` and `--config-dir=<app-owned-dir>` — the manual explicitly notes this "may be a good idea to avoid conflicts with the normal mpv user configuration intended for CLI playback."

**libmpv:** `mpv_command`, `mpv_command_node`, `mpv_set_property`, `mpv_get_property`, `mpv_observe_property` and `mpv_render_context` (for embedding video output in the app's own surface) are documented in `client.h`. Rust bindings on crates.io: **`libmpv-sys` 3.1.0, last published 2020-07-22** (34,720 downloads) — stale; **`mpv-client` 1.1.0, published 2025-06-28** (25,098 downloads) — a maintained JSON-IPC client. `libmpv-rs` was **not found** on crates.io. Recommendation: drive mpv over the JSON IPC socket with `mpv-client` (or a small hand-rolled client) rather than binding libmpv, unless in-window video rendering is required.

### 5.2 Embedded subtitle extraction with ffmpeg

List subtitle streams:
```bash
ffprobe -v error -select_streams s -of json \
  -show_entries stream=index,codec_name,codec_type:stream_tags=language,title input.mkv
```
Extract a text subtitle stream to SRT (**text codecs only** — this fails for PGS/DVB/VobSub rather than silently OCR-ing):
```bash
ffmpeg -i input.mkv -map 0:s:0 -c:s srt output.srt
```
Copy ASS/SSA preserving formatting vs. converting (`-c:s copy` is bit-exact and keeps styling and positioning; `-c:s ass` normalizes SSA→ASS; `-c:s srt` strips all styling, karaoke and positioning):
```bash
ffmpeg -i input.mkv -map 0:s:0 -c:s copy output.ass
```
ASS→SRT loses inline override tags (`\pos`, `\an`, `\k`), fonts, and sign-translation positioning — prefer `-c:s copy`. Bitmap subtitles extract losslessly to `.sup`, and text conversion requires OCR:
```bash
ffmpeg -i input.mkv -map 0:s:0 -c:s copy output.sup
```
OCR (tesseract with per-cue image cropping) is a separate, error-prone pipeline and must be an explicit user action, never automatic. The approach is recorded; no OCR pipeline was built or verified here.

Charset-related input options, all verified present in the ffmpeg docs:
- `-sub_charenc <encoding>` — documented as `sub_charenc encoding (decoding,subtitles)`: "Set the input subtitles character encoding."
- `-fix_sub_duration` — verified in `ffmpeg.html` (3 occurrences); corrects overlapping/mis-sized text subtitle durations on input.
- `-canvas_size` — verified in `ffmpeg-codecs.html` (6 occurrences); positions bitmap subtitle streams whose stored resolution differs from the video.

### 5.3 Charset detection and conversion matrix

**Charset detection.** Legacy SRT/SUB files are frequently CP1252, CP1251, CP1256, Shift-JIS, GB18030 or Big5 with no BOM. Verified on crates.io: **`chardetng` 1.0.0, published 2026-03-30** (14,626,785 downloads) — Mozilla's detector, detection only, so pair it with **`encoding_rs` 0.8.42, published 2026-09-24** (563,222,143 downloads) for the actual decode/encode. Detection improves with more bytes, so feed whole subtitle files rather than samples.

| Format | Text or bitmap | Needs OCR? | mpv renders natively? | Notes |
|---|---|---|---|---|
| SRT (SubRip) | Text | No | Yes | Universal interchange target |
| WebVTT (VTT) | Text | No | Yes | Preferred for web/HTML players |
| ASS | Text | No | Yes | Highest fidelity in mpv; carries fonts and positioning |
| SSA | Text | No | Yes | Predecessor to ASS; convert with `-c:s ass` |
| MicroDVD (`.sub`) | Text | No | Yes | Frame-number based, **fps-dependent** — a common source of drift |
| SubViewer | Text | No | Yes | |
| VobSub (`.sub` + `.idx`) | **Bitmap** | Yes | Yes (renders) | Text only via OCR |
| DVB subtitles | **Bitmap** | Yes | Yes (renders) | Common in European DVB-T captures |
| PGS (`.sup`) | **Bitmap** | Yes | Yes (renders) | Blu-ray standard; the highest-volume OCR case |
| TTML / DFXP | Text | No | Yes (via libass/ffmpeg) | XML-based; used by streaming services |
| SAMI (`.smi`) | Text | No | Yes | Legacy Microsoft format |

The architectural split: **text formats convert losslessly between each other and are editable; bitmap formats render but never become text without OCR.** Detect the codec from `ffprobe` (`codec_name` ∈ `subrip`, `ass`, `ssa`, `mov_text`, `webvtt`, `microdvd`, `dvd_subtitle`, `hdmv_pgs_subtitle`, `dvb_subtitle`) and branch *before* offering a "convert to SRT" action. mpv's libass rendering handles both classes, so the "just play it" path needs no conversion at all — conversion matters only for portable text (search, translation, external sync).

---

## Unverified / needs a spike

| # | Item | One-line experiment |
|---|---|---|
| 1 | TMDB `/tv/{id}/content_ratings` shape | `curl -H "Authorization: Bearer $KEY" https://api.themoviedb.org/3/tv/1399/content_ratings` |
| 2 | TMDB episode-groups endpoint + params | `curl -H "Authorization: Bearer $KEY" https://api.themoviedb.org/3/tv/1399/episode_groups` |
| 3 | TMDB trending param schema | `curl -H "Authorization: Bearer $KEY" https://api.themoviedb.org/3/trending/movie/week` |
| 4 | TMDB `with_release_type` numeric values | Fetch the TMDB "Finding Data" guide and enumerate 1–6 |
| 5 | TMDB sustained-throughput limit (doc says ~40 rps) | Ramp 5/10/20/40 rps for 60 s and record the first 429 |
| 6 | TVDB v4 rate limits (undocumented everywhere fetched) | With a key, hammer `/series/{id}` and record the first 429 / `Retry-After` |
| 7 | TVDB v4 artwork path grammar (v4 guess returned 403) | Read a real `image` field from `/series/{id}/extended` and fetch it |
| 8 | Uncompressed size of `title.akas` / `name.basics` / `title.principals` | `curl <file> \| gunzip -c \| wc -lc` (3 downloads, ~1.6 GB) |
| 9 | IMDb conditional re-download actually returns 304 | `curl -H 'If-None-Match: "<etag>"' -I https://datasets.imdbws.com/title.ratings.tsv.gz` |
| 10 | Trakt / Simkl / AniList ToS (none fetched) | Read each terms page; record caching and attribution clauses |
| 11 | Trakt documented limits still current (legacy blueprint); MAL v2 limit (absent from reference) | Confirm Trakt at docs.trakt.tv; send MAL 10 rapid requests and record `X-RateLimit-*` |
| 12 | OpenSubtitles full search params, download quota, free-tier attribution, `/login` shape | Fetch the Stoplight OpenAPI JSON with a valid `Api-Key` and real `User-Agent` |
| 13 | Subdl param set (Cloudflare-blocked) | Request `subdl.com/api-doc` from a browser session |
| 14 | `ffsubsync` / `alass` / `SubtitleEdit` licences + maturity (GitHub API 403'd) | Fetch each repo page; record SPDX id and last commit |
| 15 | torlink's piece prioritization / delivery design; Peeri's existence | Read `baairon/torlink` source for HTTP-range vs pipe; locate Peeri first |
| 16 | DHT bootstrap host list (BEP 5 gives only an example) | Read libtorrent/Transmission source for compiled-in defaults |
| 17 | `_googlecast._tcp` TXT field set + CASTV2 framing/namespaces | `dns-sd -B _googlecast._tcp` on a LAN with a Chromecast, then `protoc --decode_raw` one session |
| 18 | DLNA DIDL-Lite skeleton, `protocolInfo` strings, `ContentDirectory:Browse` | `GetProtocolInfo` against the Samsung TV at `192.168.1.50:9197`; re-run SSDP with `ST: ssdp:all` plus a NAS |
| 19 | External-IP leak-check endpoint list; split-tunnel + interface-bind semantics per OS | Compare 3–4 echo services; kill the VPN mid-run and assert all peer I/O stops (fail-closed) |
| 20 | YouTube ToS verbatim download-prohibition clause; Dailymotion limits/ToS | Read the API Services ToS + Developer Policies; query the Dailymotion developer portal |
| 21 | TRaSH Guides current release-profile regex page (old Sonarr path returns 450 bytes) | Find the live regex page from `trash-guides.info` navigation |
| 22 | Tesseract OCR accuracy on PGS subtitles | OCR one 1080p `.sup`; measure character error rate against a known script |
| 23 | `rust_cast` media-namespace completeness (LOAD/SEEK/status) | Build a 20-line load-and-seek test against a real Chromecast |
