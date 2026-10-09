# ADR 0019 — Local accounts, avatars, and parental gating

- **Status:** Accepted as **post-1.0 roadmap** (Phase 8). One exception is pulled **into 1.0**: the account-scope seam in the query layer, because retrofitting gating into a finished read path is the expensive version of this feature.
- **Date:** 2026-10-09
- **Evidence:** `objc2-local-authentication` 0.3.2 (2025-10-04, Zlib OR Apache-2.0 OR MIT) for macOS `LAContext`; `windows` 0.62.2 (MIT OR Apache-2.0) for `UserConsentVerifier`; `zbus` 5.19.0 (MIT) for Linux D-Bus; `polkit` 0.19.0 (0BSD OR Unlicense); `pam` 0.8.0 (2023-11-01). All resolved 2026-10-09. No cloud service is involved anywhere in this design.
- **Verification:** `SPIKE-17` (OS authentication reality per platform), `SPIKE-18` (gating completeness and leak audit)
- **Related:** [0002](./0002-rust-core-owns-state.md), [0015](./0015-native-notifications.md), [0017](./0017-mcp-server.md), [0018](./0018-media-server.md)

## Context

The requirement is Netflix-style accounts in a household: several people, each with their own avatar, their own content, and their own history; a dedicated child account whose content a parent curates; a child account that **cannot download**; and switching out of the child account gated by whatever the device offers — Touch ID, Windows Hello, the OS password. Explicitly local: no account server, no email, no cloud identity.

Three constraints shape the design.

1. **This is all local, and local means the adversary may share your OS login.** If a child uses the same operating-system account as the parent, then the database, the app's configuration, and the keyring are all readable by them. Anything we call "parental control" that lives only inside a process we also hand to the child is a *usability* boundary. Saying otherwise would be dishonest, and hiding that fact would be worse.
2. **Content gating must live in the query layer, not the UI.** A filter in React means the blocked titles are still fetched, still cached, still counted, and still revealed by search suggestions, "similar titles", notification bodies, and the media server's item counts. Gating has to be a property of reads in the Rust core.
3. **The OS already has the parent's identity.** Biometrics and Hello are bound to a real person and cannot be replayed by a child sharing the login. An app-level PIN can be guessed, shoulder-surfed, or read from the database if we are careless. So OS authentication is the primary gate and an app PIN is the documented fallback, never the default.

## Decision

### 1. One local account per person, with no network identity anywhere

`account` (renamed from `profile`, since "profile" already means a quality profile in this schema) holds identity, content scope, capabilities, and lock policy. There is no registration, no email, no password reset over a network, no avatar upload, and no analytics about who watches what. Avatars are either an image the user picks from disk or a generated mark; both are stored locally with a blurhash placeholder, exactly like poster artwork.

Account kinds: `adult`, `child`, and `guest`.

**Guest is kept, as exactly one account named Guest.** Not one per visitor and not a named session: a single shared account, created once by an adult, with a name that is not editable and rendered from a translation key so it reads correctly in every language. Nothing personal is attached to it — no custom avatar, no per-visitor identity — because a shared account should not accumulate anyone's data. It never gains capabilities, it retains no watch history once its session ends, and it is deliberately gate-free: the whole point is that a visitor can use it without the household's credential. That creates one obvious question: can a child escape into guest? Only if guest is *broader* than the child's own scope, which would be a configuration hole rather than a mechanism failure. So the rule is structural:

> **An unauthenticated switch may only narrow what is visible.** Switching to any account whose effective scope is broader than the current one requires the gate; switching to a narrower or equal scope does not.

Guest is therefore safe by construction even when enabled: its ceiling defaults to the most restrictive child account's, and an adult who widens it does not create an escalation path, because a child session can never reach a broader scope without authenticating.

### 2. Content visibility is enforced in the core, fails closed, and leaks nothing

An `AccountScope` is threaded through every read of catalogue, library, episode, progress, watchlist, notification, and search data. The default single-account installation carries an unrestricted scope, which is why the seam exists in 1.0 even though the feature does not.

| Rule | Detail |
| --- | --- |
| Rating ceiling | Per account, using the `content_rating` we already store from TMDB release dates and content ratings |
| Curation | Allow-lists for titles and collections — the parent's "this is what you may watch" — plus deny-lists for titles, genres, and keywords |
| Roots | Which library roots an account may see at all |
| Fail closed | Content with no rating, or with a rating we cannot map, is **hidden** from a restricted account until a parent allows it explicitly. An unrated title is not "probably fine" |
| Indistinguishable from absent | A blocked title returns the same result as a nonexistent one: not in search, not in counts, not in "similar", not in continue-watching, not in a notification body, and no different error message. Blocked content is not merely filtered out of a list; it does not exist for that account |
| Defence in depth | The player and the media server re-check the scope at request time, so a stale cache or a hand-crafted request cannot play something the account cannot see |

### 3. Capabilities, not just content

| Capability | Adult | Child | Guest |
| --- | --- | --- | --- |
| Browse and play within scope | yes | yes | yes |
| Add to own watchlist | yes | yes | no |
| **Request** a download | yes | yes | no |
| **Create** a download | yes | **never** | no |
| See the download queue | yes | no — the queue reveals what adults are acquiring | no |
| Delete media, change settings | yes | no | no |
| Cast to a device in the house | yes | yes, within scope | no |
| Grant an agent (MCP) access | yes | no | no |

A child account cannot acquire, and the refusal is a typed core error rather than a hidden button, so no UI path, deep link, agent call, or media-server request can reach it.

### 4. The child asks, the parent approves

Requests are a first-class object rather than a permission prompt:

1. The child selects a title and taps *Ask*.
2. The parent gets a notification (per the ADR 0015 catalogue, a new `request.created` event, sound on by default) and sees the request in a queue.
3. Approving runs the normal acquisition path — safety gate, ranking, profile, limits — and records who approved it and for whom.
4. Denying states a reason the child sees, because "no" without a reason invites the behaviour we are trying to avoid.

This reuses the two-phase approval machinery built for MCP ([adr/0017](./0017-mcp-server.md)), which is the same problem: a request that spends resources and needs a human decision.

### 5. Switching accounts is gated by the OS, through a port

`OsAuthenticator` is a port in the core with one adapter per platform:

| Platform | Primary | Fallback | Verified crate |
| --- | --- | --- | --- |
| macOS | `LAContext` with device-owner authentication: Touch ID, Apple Watch, or the device password | App PIN | `objc2-local-authentication` 0.3.2 |
| Windows | Windows Hello via `UserConsentVerifier::RequestVerificationAsync`, after `CheckAvailabilityAsync` | App PIN | `windows` 0.62.2 |
| Linux | polkit authentication agent prompt where one is running | fprintd via D-Bus, then app PIN | `zbus` 5.19.0, `polkit` 0.19.0 |

Rules:

- **Both directions are gated.** Leaving a child account requires authentication, and entering an adult account requires it too. Either alone leaves an obvious hole.
- **Auto-lock.** The app returns to the account picker after a configurable idle period, on explicit lock, and when the last user of a child account quits.
- **Rate limited and audited.** Attempts are recorded with method and outcome; repeated failures back off exponentially. The audit is visible to the parent.
- **No bypass exists.** There is no "reset parental controls", no recovery code, and no support path that skips the OS check. Losing the OS credential means losing access to that account, which is the correct failure mode.
- **If nothing is available** — no biometrics enrolled, no polkit agent, a Linux desktop with neither — the account still works but the parent is told plainly that the gate is an app PIN, which is weaker, and that separate OS accounts are the real fix.

### 6. The boundary, stated in the product and not only here

With a shared OS login, this feature stops **casual account switching**: a child cannot open the app and pick the adult account. It does not stop someone who can read the app's files as that OS user, and it cannot, because the same user owns the database.

So the UI says it once, in the parental settings, without euphemism:

> This stops someone from switching accounts in Peregrine. It does not protect the files on this computer. For that, give each person their own computer account.

And the documentation recommends separate OS accounts as the only real separation, with Peregrine's accounts as the convenience layer on top. We do not implement OS-account detection as a security claim; if we detect that a child and a parent share a login, we say so and nothing more.

### 7. Server and agent requests carry the account

A paired casting or media-server device ([adr/0018](./0018-media-server.md)) and a paired MCP client ([adr/0017](./0017-mcp-server.md)) are both **bound to an account**. A child's tablet sees the child's scope; a child's agent cannot acquire on their behalf. Without this, both features would be a documented bypass of everything above.

### 8. Avatars are local, original, and chosen when the account is made

Account creation is name plus avatar, and it happens in the first-run setup for the first account and from the switcher for every account after that:

| Avatar source | Detail |
| --- | --- |
| **Built-in set** | A small gallery of original marks shipped with the app, drawn in the same geometric language as the logo. Ours, so no third-party icon pack enters the build |
| **Initials** | The account's initial set over a themed shape in the account's accent colour, generated at render time — the zero-effort default that still looks intentional |
| **Custom image** | Chosen from disk, validated and normalised on import: PNG, JPEG, or WebP only; rejected if it is an SVG or animated; input under 10 MB and 4096 px; **EXIF and GPS metadata stripped** because a photo of a child usually carries a location; decoded through the same bounded image path as provider artwork; square-cropped and resized to the six avatar sizes; blurhash computed for instant paint |
| **Generic default** | A neutral silhouette, used when nothing else is chosen. Guest uses it permanently and cannot change it, since a custom avatar would attach a person to a shared account |

Name rules are deliberately light — non-empty, trimmed, 32 characters, unique within the installation — because the name is displayed, not used as an identifier. No password is ever stored for an adult account: the OS credential is the gate, and an app PIN exists only as the documented fallback.

The avatar set is part of the brand assets, so it carries the same licensing discipline: original work, attributed in `NOTICE`, and no third-party icon pack. `SPIKE-18` additionally checks that a custom avatar cannot become an injection vector — no SVG, no animated format, no unbounded decode.

## Consequences

Positive: several people can share one installation without seeing each other's content or history; a child account is genuinely incapable of acquiring media rather than merely lacking a button; the parent curates by collection and rating rather than by micromanaging; and the gate uses the strongest local credential available instead of an app PIN we would have to invent.

Negative and accepted:

- **The security boundary is the OS login, not this feature.** Stated in the UI, in SECURITY.md, and here. Anyone who claims otherwise is selling something.
- Gating every read path means the scope parameter is threaded through the repository layer and re-checked at play time. That is real work and a source of subtle bugs, which is why the seam lands in 1.0 and `SPIKE-18` audits it.
- Linux has no dependable biometric API; polkit and fprintd cover the common desktops and the app PIN covers the rest, with an honest warning.
- A forgotten OS credential is unrecoverable by design. No backdoor, no support override.
- The `profile` → `account` rename touches the schema, the invariants, and several documents. Cheaper now than after any of it ships, which is the only reason to do it today.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Profile-style separation without content gating (history only) | Does not answer the actual request: a child would still see and play everything |
| Filter in the UI only | Blocked titles stay in the cache, the counts, the suggestions, and the notification bodies. Worse than no filter, because it implies safety |
| An app-only PIN as the primary gate | Guessable, shoulder-surfable, and readable from storage by a determined child. Acceptable as a fallback, not as the design |
| Encrypt the database with a parent-held key | The key must be available to the running app under the same OS user, so it buys little, while breaking backup, portable mode, and inspection. It would look stronger than it is |
| Cloud accounts with a real login | Contradicts local-first, adds a server and a privacy surface, and creates a password-reset path that bypasses the parent |
| Bundling a third-party avatar pack, Netflix-style icon sets included | Those are someone else's artwork and brand. We ship an original set instead, which is also why the avatar gallery is small rather than exhaustive |
| Accepting SVG uploads for avatars | An SVG is a script container, and an avatar renders inside a privileged window. Raster formats only, decoded through the bounded image path |
| Keeping EXIF in imported avatars | Camera metadata frequently includes GPS coordinates, which is exactly the wrong thing to keep alongside a child's profile picture |
| One guest account per visitor, with the visitor's own name | Puts a real person's name into the database, the audit log, and the UI for a shared convenience account. A single Guest is simpler and keeps less |
| Making Guest renameable | A shared account whose name is a fixed, translated label cannot be mistaken for a person's account, and it removes one more thing that can silently become personalised |
| Hiding the adult accounts from the picker entirely | The parent still needs to reach them, and a hidden account is security theatre: the switcher's existence is not the weakness |
| Detecting "the child is using the parent's OS login" and refusing to run | Unenforceable, annoying, and it would break the common family setup for no real gain |
| Letting a child cast anything an adult can | Casting is how a child watches on the TV; without scope binding it becomes the bypass. Binding the device to the account closes it |

## Verification

`SPIKE-17` (2 days): confirm on real hardware that Touch ID, Apple Watch, and the device password each satisfy the macOS gate; that Windows Hello works with PIN, fingerprint, and face and reports unavailability cleanly; and what a GNOME and a KDE desktop actually offer — polkit agent, fprintd, both, or neither — including the failure message when neither exists.

`SPIKE-18` (3 days): an audit that every read path is scoped, with a permanent test set that attempts to reach blocked content through search suggestions, similar titles, continue-watching, notification bodies, direct item IDs, the casting server, the Jellyfin API subset, and a hand-written MCP call. Any leak fails the suite.
