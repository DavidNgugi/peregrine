# ADR 0032 — Shared profiles on the account table

- **Status:** Accepted. Supersedes [0027](./0027-local-profiles.md).
- **Date:** 2026-10-20
- **Related:** [0019](./0019-local-accounts-and-parental-gating.md), [0028](./0028-tv-web-client.md), [0031](./0031-browser-client-parity.md)

## Context

ADR 0027 made the picker a device-local list of labels. That kept the shell usable before accounts,
but it produced exactly the wrong answer for a household: a profile added in a browser did not
appear on the desktop, an avatar did not travel, and switching did not change the watchlist or
progress because every read ran under the same installation scope.

The account table already exists with names, avatars, kinds, and capability columns. The library is
shared by design; the per-person state is already keyed by `account_id` in watch progress, the
watchlist, continue-watching, notifications, and history.

## Decision

1. **A profile is an `account` row.** The list, names, avatars, kinds, rating ceilings, curation
   flag, and acquisition capability live in the database, so the desktop and every paired browser
   read the same household.
2. **One active profile is installation-wide.** The choice is stored in `settings` under
   `active_account`, and the runtime's scope resolves that account. A profile selected in the
   browser reloads that client; the desktop reloads when it observes the change. This is a shared
   living-room selection, not a per-device session.
3. **The library is shared; personal reads are scoped.** Catalogue and library visibility still take
   `AccountScope`, now resolved from the active account. Watch progress, the watchlist,
   continue-watching, history, and notifications are keyed by that account. Two profiles see the
   same titles and different "what I was watching" and "my list".
4. **The avatar is a stored data URL.** The desktop's DiceBear or uploaded picture is written to
   `avatar_ref` at 128 px, so a browser draws the same picture without a second avatar system. A
   profile created without one draws its initials and a hue derived from its name.
5. **Kid provisions are in the row, not yet a gate.** `kind = child` carries a ceiling and
   `can_acquire = 0`. The scope hides adult and unrated titles, and the acquisition routes refuse a
   profile whose row says no. The PIN/OS gate, per-rating mapping, and curation editor remain E35.
6. **No delete.** A profile is removed with the account row only after its watch state is dealt with;
   the picker does not offer a destructive action yet.

## Consequences

- The active choice is not per-device; two people cannot watch different profiles at the same time
  on two screens without changing the shared setting. Per-device selection is a later refinement of
  this record, not a second profile store.
- The web client offers the desktop's editor: a profile created or renamed in a browser picks one of
  the sixteen DiceBear styles or uploads a square 128 px image, and the picture is written to
  `avatar_ref` through the shared runtime. Remaining E35 work is a dimension cap, a content-level
  refusal of SVG and animation, and the parental gate.
- Existing local profile books are not migrated: the account table's seeded `You` is the first
  profile, and profiles created after this change are the shared ones.
- A kid profile is a usability boundary today, not a security boundary: anyone who can reach the
  picker can switch. E35's gate is what makes switching out of it require the parent's authority.
