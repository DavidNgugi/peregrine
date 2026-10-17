# ADR 0027 — Local profiles before accounts

- **Status:** Accepted. Provisional until [0019](./0019-local-accounts-and-parental-gating.md) lands.
- **Date:** 2026-10-18

## Context

The shell needs a place to choose who is watching, and the roadmap's local accounts and parental
gating are phase 8. Building the selector as a route gated on accounts would leave the product with
no entry point until that work lands; building a second, real account model now would duplicate what
0019 already decides.

## Decision

A profile is device-local and deliberately thin: an id, a display name, a hue used as a fallback
colour, and an avatar. The avatar is either a DiceBear drawing (style and seed) or a 256 px JPEG data
URL from an uploaded file. The book of profiles is stored in `localStorage`, the selector is a
full-window modal reachable from every screen, and the app reports one active profile to the sidebar
and the Discover toolbar.

**It is not a security boundary.** It does not scope the catalogue, the library, watch progress,
downloads, or settings. A child profile would see everything the adult profile sees. The UI says the
profile is stored on this device, and the editor's avatar attribution names DiceBear.

## Consequences

- Accounts can replace the book without changing the selector's shape: the modal already takes a
  list, an active id, and select/add/rename callbacks.
- Uploaded avatars live in `localStorage` as compressed data URLs, not in the app's data directory.
  They are small (tens of kilobytes) but they are subject to that storage's quota, and clearing site
  data clears them.
- Profile switching cannot hide content, and nothing in the UI claims it can. Parental gating and
  scoped reads remain 0019's work.
- DiceBear is a new frontend dependency, pinned with its collection so the core and the styles agree;
  the editor's footer carries the attribution the CC BY styles require.
