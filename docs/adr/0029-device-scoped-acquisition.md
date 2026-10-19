# 29. Device-scoped acquisition from the LAN client

- **Status:** Accepted
- **Date:** 2026-10-19
- **Supersedes:** the read-only sentence in [ADR 0028](./0028-tv-web-client.md) §3, for the narrow case below
- **Evidence:** `crates/peregrine-app/src/web`, `crates/peregrine-storage/migrations/0024_web_audit.sql`, and PG-E37-11

## Context

ADR 0028 gave a paired browser or television `read,stream` and no acquisition surface. That is the right default: a leaked television token should not be able to spend bandwidth or disk. It also makes the client a viewer of whatever the desktop already acquired, which is not enough for a catalogue title the user has just found on the television.

The request is a **download button on the title page**, gated by a permission the person grants at pairing. It is not a general write surface: no delete, settings, keyring, or arbitrary path becomes reachable.

## Decision

1. **A separate `acquire` scope.** The default remains `read,stream`. The desktop's pending-device row carries an unchecked **Allow downloads** box; only an approval with that box ticked issues `read,stream,acquire`. The code-approval path defaults to no acquisition. Existing devices stay read-only; changing one means revoking it and pairing again, which is the one place a person sees the checkbox.
2. **The client asks what it may do.** `GET /api/me` answers the authenticated device's name, scopes, and `canAcquire`. The title page draws a Download button only when that is true, and the server independently enforces the scope on the write.
3. **One acquisition write.** `POST /api/acquire` takes `{titleId, season?, episode?}`. A season and episode are given together or neither; the title and episode must exist. It calls `Runtime::start_acquire` with the installation scope and the minimum-seeders default the desktop uses. Quality choice, the safety gate, disk floors, and the seeding policy are the existing job, not a second path.
4. **Audit before the queue.** A `web_audit` row records the device, `acquire`, the title, the episode when known, and the instant, and is written before the queue is touched so a failed submission is still attributable. When the queue accepts it the journal id is attached. Revocation keeps the row, just as the existing device rows survive revocation.
5. **No new network posture.** The call is same-origin, cookie-authenticated, JSON, and behind the existing `Host`/`Origin` gate and body cap. There is no CORS, no token in a URL, and no WAN exposure.

## Consequences

- A stolen `acquire` token can spend bandwidth and disk. That is the explicit trade the checkbox makes; the alternative is no browser acquisition. The audit names the device, and revoking it is immediate.
- A television approved for downloads still cannot change a quality profile, choose a release, delete a file, or read a path. It can ask the same automatic acquisition a person can ask from a title's Download button on the desktop.
- The web title page offers **Download film** for a missing film, **Download series** when a series has no episodes yet, and a **Download** button on each episode that has no playable file. Once a file imports, Play replaces that button; progress remains visible in Downloads until the queue finishes.
- Tests cover the default refusal, the scope at approval, `/api/me`, the invalid episode shape, and the audit write. A real release search is the same core test the desktop uses; no test downloads from the internet.
