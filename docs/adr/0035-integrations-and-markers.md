# ADR 0035 — Integrations and playback markers

- **Status:** Accepted.
- **Date:** 2026-10-25
- **Related:** [0007](./0007-metadata-sources.md), [0023](./0023-in-app-video-surface.md), [0028](./0028-tv-web-client.md), [0031](./0031-browser-client-parity.md), [0033](./0033-account-backup-and-restore.md)

## Context

The first integration was TMDB, and it grew its own settings section, its own credential job, and
its own registry entry. The next sources are not interchangeable with it: TheIntroDB and AniSkip
need no key to read, TheIntroDB's key is for submissions only, and a marker source is asked at
playback time with the file's duration rather than during a catalogue sync. A second bespoke
section per source would multiply the places a credential can be mishandled.

Playback markers have the same shape problem. Chapters, TheIntroDB, AniSkip, and IntroHater can all
answer for one episode; they disagree, some are wrong for a particular cut, and a wrong skip is
worse than no skip. The renderer cannot choose between them: it has no database, no provider
credential, and no clock that a cache can be compared against.

The product requirements are two: one Integrations screen where a person enables, credentials,
tests, and orders each source; and one player experience in which a marker is skipped only when a
marker exists.

## Decision

1. **One manifest owns the catalogue.** `peregrine-app::integrations::MANIFEST` is one table entry
   per adapter that exists: id, category, capabilities, credential fields, submission support,
   whether it is always on, default priority, and how to probe it. The desktop card, the TV status,
   the credential job, and the marker resolver read that table. An id not in it is refused before a
   job exists.

2. **A read may need no key, and the manifest says so.** `requires_key` is derived from the required
   fields, not entered twice. TheIntroDB's `apiKey` is optional and its absence is `configured`,
   because public reads work; AniSkip has no field at all. TMDB's field is required. A source with no
   required field and no credential is the normal case, not an error.

3. **Credentials live in the OS keyring, one entry per field.** The database stores only
   `integration_setting`: the choice, the priority, submission consent, and the last probe's health.
   TMDB keeps its existing `provider.tmdb` account so an installation's key is not lost; every new
   field is `integration.<id>.<field>`. A value is written by a job, never returned by a command,
   and the journal carries a one-use token instead of a value. A denied or unavailable read is a
   distinct state, never "not configured".

4. **A marker belongs to a title or an episode, not a file.** `playback_marker` is keyed by
   `(title_id, episode_id, kind, source, start_ms)`; `marker_fetch` caches a positive, negative, or
   failed lookup with a TTL. The domain resolves candidates per kind: chapters outrank every network
   source, the confidence floor drops a weak network marker, overlapping intervals from one source
   merge, and the runtime bounds every marker. A suppressed row is local, is never submitted, and is
   filtered before the player sees it.

5. **The refresh runs after the session opens.** A play answers from the cache; a background task
   asks chapters and the enabled network sources, writes the cache, and publishes
   `player://markers`. No provider call is on the first-frame path, and no renderer calls one. The
   desktop session and the LAN ticket both carry the cached markers and the next playable episode.

6. **The desktop edits; the TV reads.** The LAN surface exposes `GET /api/integrations` with the
   category, enablement, configured flag, and the last outcome. It has no credential field, no
   present flag, and no write. "Manage on desktop" is the only instruction it can carry
   (0028/0033). The same rule makes the backup safe: a backup copies the database, the keyring is
   not in it, and a restored installation reports "key needed" because the store answered with
   nothing.

7. **Post-play is one coordinator, not five listeners.** The shared `player-ui` package owns the
   countdown state machine: a credits marker or the end of media opens it, a next episode starts
   exactly once at zero, Cancel is safe, a film ends on a panel, and a prefetch failure falls back
   to a normal load. The desktop and web players use the same reducer and the same
   `PlayerSnapshot`/`PlayerCommand` vocabulary; each engine adapter is a separate concern.

## Consequences

- The first slice ships chapters, TheIntroDB, AniSkip, and TMDB. OpenSubtitles, Subdl, IntroHater,
  and Trakt are not in the manifest, because no adapter can be called. Adding one is a manifest
  entry, an adapter, and a probe — not a second settings screen.
- TheIntroDB's optional key is proven only by a public read. A key that the service rejects only for
  submissions cannot be detected until a submission UI exists; the setting says "optional" rather
  than claiming a verified submission.
- A marker source that is disabled is not asked, and its cached rows remain: re-enabling it does not
  need a refetch until the TTL expires. A source that is deleted from the manifest leaves its rows
  in place; they are filtered by the resolver's source table and can be pruned with the marker cache.
- The external-mpv hand-off still owns its own window, so a container the webview cannot demux has
  no React skip button in this slice. The desktop player's skip controls are drawn only for the
  in-page surface; mpv parity remains a named limitation, not a claim.
- Local suppression has storage and resolution but no settings surface in this slice. The row and
  the rule are testable; the control is a follow-up.
- The first-frame contract is unchanged: a slow provider cannot delay a play. The price is that a
  marker discovered after the first frame appears only when the player re-reads the cache.

## Alternatives considered

- **One bespoke settings section per provider.** Rejected: it repeats the credential path, and the
  TV status has no single answer to "which marker sources are on".
- **Fetch markers from the renderer.** Rejected: the browser would hold credentials, duplicate the
  cache, and put a provider on the frame path.
- **Store credentials in the database beside the setting.** Rejected by SECURITY T13 and 0033: a
  backup would carry them, and a diagnostics read would expose them.
- **Let the provider with the highest priority answer the whole episode.** Rejected: chapters can
  win the intro while AniSkip wins the ending, and a single winner throws away a correct marker.
- **Autoplay the next episode immediately on `ended`.** Rejected: it removes the cancel window and
  makes a wrong next-episode choice unrecoverable without leaving the player.

## Verification

- Manifest: every id is valid and unique; TheIntroDB and AniSkip are configured with an empty
  store; TMDB is not; chapters is enabled by default and a network source is not.
- Keyring: a denied read is recorded as denied and not as an empty store; a later refusal does not
  forget a key a previous read found; a parked candidate is taken exactly once.
- Storage: markers round-trip, a refetch keeps a suppressed row, a negative fetch expires, and a
  subject carries its external ids and anime flag.
- Domain: chapters beat the network per kind, a weak marker is dropped, overlaps merge, a marker
  past the runtime is dropped, and only credits may be open-ended.
- Adapters: TheIntroDB and AniSkip are tested against recorded shapes with no live network; 401,
  429, 5xx, malformed JSON, an oversized body, and an empty answer each map to the documented
  outcome.
- Player: the post-play reducer starts exactly once, cancel starts nothing, a film ends on the
  panel, and a prefetch failure does not remove the next episode.
- Platforms: the web and TV read the status but have no credential input; the desktop has the only
  editor.
