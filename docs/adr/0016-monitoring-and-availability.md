# ADR 0016 — Monitoring: watching upcoming releases and new episodes

- **Status:** Accepted as **post-1.0 roadmap** (Phase 6)
- **Date:** 2026-10-09
- **Evidence:** [research/data-sources.md](../research/data-sources.md) — air-date and calendar sources, cadence table, and the "what realtime actually means" section, all verified live
- **Related:** [0007](./0007-metadata-sources.md), [0008](./0008-incremental-polite-sync.md), [0015](./0015-native-notifications.md), [0017](./0017-mcp-server.md)

## Context

The requirement is a system that keeps watch on specific movies and shows — upcoming releases, new episodes — and tells the user when something becomes available. This is a different problem from discovery: it is per-item, long-lived, change-detecting state that has to survive restarts, app upgrades, and a month of the app being closed, and it has to notify exactly once per real event without becoming noise.

The verified source research constrains the design:

- **No free API offers push.** The best cadences are: TMDB `/changes` and `/tv/changes` on a **24-hour floor** (polling faster cannot observe anything new), TVDB `/updates?since=` which is genuinely continuous, Trakt and Simkl calendars at roughly 6 hours, indexer RSS at 15–30 minutes, and IMDb datasets daily.
- **Air date and availability are different things.** An episode airing is not the same as a release being obtainable, and neither is the same as it being in the library. TMDB's `release_dates` even distinguishes release *types* (premiere, theatrical, digital, physical), so "the digital release is out" is knowable rather than guessed.
- **The fastest real signal that something is obtainable is the indexer**, not the metadata provider. Release appearance beats metadata propagation by hours to days.

## Decision

### 1. A monitor is a persisted, scoped watch with an explicit state machine

Scope is one of `movie`, `show`, `season`, or `episode`. One row per monitored target, carrying the sources to poll, the notification policy, the acquisition policy, and the quality profile.

```
scheduled ──► airing ──► released ──► release_found ──► in_library
    │            │           │              │
    └────────────┴───────────┴──────────────┴──► paused | ended | blackholed
```

| State | Meaning |
| --- | --- |
| `scheduled` | Known to be coming; no air or release date has passed |
| `airing` | A season is in progress, or an episode date has passed without the next signal |
| `released` | The air date or the chosen release type's date has passed, but no obtainable release is known |
| `release_found` | At least one release meeting the account's floor was seen |
| `in_library` | Imported and probed; `in_library` is terminal until a file is deleted |
| `paused` | User suspended watching; no polling, no notifications |
| `ended` | The show concluded, or the movie's release window closed, with nothing left to wait for |
| `blackholed` | The user gave up on this target; kept so we never re-suggest it |

Only **state transitions** notify. A monitor that is already `release_found` does not notify again because a second release appeared.

### 2. Polling is per-source and never faster than the source can change

| Source | Cadence | What it answers |
| --- | --- | --- |
| TMDB episode air dates for monitored shows | 12 h | What airs next, and whether a date moved |
| TMDB `release_dates` for monitored movies | 12 h | Whether the digital or physical release window opened |
| TVDB `/updates?since=` (when a key is present) | 30 min | Genuinely incremental change detection, no 24-hour floor |
| Trakt or Simkl calendars (optional) | 6 h | What airs tonight or this week, as a cross-check |
| Indexer RSS for monitored targets | 15–30 min | Whether something is now obtainable — the fastest real signal |
| TMDB `/changes` + daily ID exports | 24 h | New titles and metadata drift |

Monitors are grouped so one poll covers every target that shares a source and a due time. A monitor in `paused` or `ended` costs nothing. Backoff on a provider failure is shared with the sync engine rather than duplicated: monitoring enqueues jobs, it does not run its own scheduler ([adr/0008](./0008-incremental-polite-sync.md)).

### 3. Availability is a derived, explainable fact

A monitor's `release_found` state requires a release meeting the account's minimum score, with seeders above the configured floor. We do not claim availability from an air date alone; the UI says *aired, no release yet* rather than implying we can get it. Every transition records the evidence that caused it — the provider field, the release title, or the RSS item — so "why did it say this is available?" has an answer.

### 4. Notification events

Five additions to the catalogue in [adr/0015](./0015-native-notifications.md):

| Event | Default | Sound | Notes |
| --- | --- | --- | --- |
| `monitor.episode_aired` | on | off | Batched into one evening digest, never one toast per episode |
| `monitor.episode_available` | on | on | A release meeting the account was found |
| `monitor.movie_released` | on | off | The chosen release type's date passed |
| `monitor.movie_available` | on | on | A release meeting the account was found |
| `monitor.acquired` | on | on | Auto-acquire completed and the item is in the library |

Dedupe key is `(monitor, subject, event kind)`, so a re-poll that re-derives the same state never notifies twice. The daily digest is the default carrier for `*_aired` events, which is what keeps a 20-show watchlist from becoming a metronome.

### 5. Acquiring is a separate, opt-in step per monitor

Monitoring is watching; acquisition is spending bandwidth and disk. A monitor can be notify-only (default), or auto-acquire under its quality profile, at which point the existing acquisition path applies unchanged — safety gate, ranking, queue, and limits. Auto-acquire never bypasses the safety gate, and it respects the global queue and free-space floor. When limits are hit, the monitor records `release_found` and notifies rather than hammering.

### 6. The calendar is the human-readable form of the same state

A calendar screen shows upcoming air dates and releases, plus three derived lists that make missing things visible: **aired but not in the library**, **in the library but below the quality cutoff** (the upgrade candidates), and **monitors that have been waiting longer than expected** (a show with no episode for 60 days is worth surfacing). These are projections of the monitor state, not a second source of truth.

## Consequences

Positive: a watchlist becomes genuinely useful instead of a bookmark list; the notification is evidence-based rather than a guess; polling is bounded by what each source can actually report, so a 20-show watchlist is a handful of requests per day rather than a hammer; and the state machine makes "what is it waiting for?" answerable in the UI.

Negative and accepted:

- **Signal latency is hours, not seconds.** An episode airs, then a release appears, then we see it. The UI states cadence per source rather than implying instant awareness.
- Monitoring depends on indexers for its fastest signal, which means a user with no indexer configured gets metadata-accurate but slower notification. That is documented, not hidden.
- Air dates move and providers disagree. The reconciler prefers the primary provider, records disagreements, and never notifies twice for the same date change.
- Long-lived state needs migration care: a monitor surviving several app versions is a real compatibility surface, and it is part of the migration fixture set.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Reuse the watchlist entry as the monitor | The watchlist is a user-facing collection with ordering and no polling semantics; conflating them makes both worse. A monitor references a title; a watchlist entry is an opinion about a title |
| Poll everything frequently | The 24-hour floor is inherent to `/changes`, and indexer endpoints return real `429`s when hammered. Frequent polling would produce identical state and worse standing with providers |
| Notify on every metadata change | Notification spam, and no user value: nobody wants to be told the vote average moved |
| Treat "aired" as "available" | Misleading, and the source of the "why can't I download this" complaint. The distinction is the feature |
| Auto-acquire by default | Spending someone's bandwidth and disk without asking is the fastest way to lose their trust |
| Build our own air-date database | Duplicating providers, and the providers are already the authority |

## Verification

- Fixture tests over air-date changes, date moves, and source disagreements, asserting exactly one notification per real transition.
- A 20-monitor watchlist must fit inside the existing provider rate limits with the documented cadence, proven by a load test that counts requests.
- A monitor persisted across an app upgrade must migrate, with the fixture set extended for each release.
- `SPIKE-13`: measure real-world latency from air date to `release_found` with and without an indexer, on ten monitored shows, so the UI copy can state honest expectations.
