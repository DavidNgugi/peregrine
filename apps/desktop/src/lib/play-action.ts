/**
 * What a primary Play action is, and what a title's own page offers beside it.
 *
 * **In `lib/` rather than in the feature that needed it first.** Two screens offer the same action —
 * the Discover hero, and a title's own page — and a rule two features share lives between them
 * (`AGENTS §5`) rather than in whichever one asked first. Both are pure functions with their own
 * tests for the reason every other shared rule here is: this harness renders to static markup in a
 * Node environment with no DOM (`vitest.config.ts`), so a rule inside a component is a rule no test
 * runs.
 *
 * **The two screens read different facts, and that is deliberate.** Discover's rail item carries a
 * boolean — the library's answer to "is there a copy" — because a rail of twenty tiles is not worth
 * the whole availability derivation. A title's own page carries
 * `peregrine_domain::TitleOffer`, which the core derives from the title's items, their files, and
 * its jobs, and which is the only thing that may decide whether a Download control is drawn at all:
 * a page that offered acquisition for a title the library holds would be asking a user to fetch a
 * second copy of what they own.
 */

import type { LocalPlaybackDto, TitleOfferDto } from "@peregrine/ipc-client";

/** Why a primary action cannot be taken. */
export type PlayReason = "noFile" | "fileMissing" | "arriving" | "noPlayer";

/**
 * What Play is: an action, or a control that is disabled for a stated reason.
 *
 * `noFile` is the core's fact about *this title* — the library holds no file for it — and it is
 * answered first, because it is the one a person can do something about. `noPlayer` is the build's
 * fact, and it is passed in rather than assumed: a caller that has no handler to give would otherwise
 * produce the inert button `PG-E1-07` forbids.
 *
 * **`containerUnknown` is not a refusal, it is an admission.** A file whose name states no container
 * this build recognises is a file the webview might well play — the decoder is the platform's, and it
 * is not limited to the formats this build can name. Refusing it would withhold playback that works;
 * promising it silently would make a failure look like a bug. So the action is allowed with a sentence
 * beside it (`ADR 0023` §5).
 */
export type PlayAction =
  | {
      readonly kind: "play";
      readonly containerUnknown: boolean;
      /** Who opens the file: the page's own media element, or mpv for a container it cannot read. */
      readonly reader: "webview" | "external";
    }
  | { readonly kind: "unavailable"; readonly reason: PlayReason };

/** The action, from the two facts that decide it. */
export function heroPlay(playable: boolean, player: boolean): PlayAction {
  if (!playable) {
    return { kind: "unavailable", reason: "noFile" };
  }
  if (!player) {
    return { kind: "unavailable", reason: "noPlayer" };
  }
  // A rail tile carries no container, which is not the same as a container this build cannot name: the
  // absence is on the *read*, and a tile that said "might not play" about every film would be saying
  // nothing. The title page is where the file itself is known, and it is the screen that admits it.
  return { kind: "play", containerUnknown: false, reader: "webview" };
}

/** What a title's own page draws, from the core's offer and the build's player. */
export interface TitlePageActions {
  /** The primary action: enabled, or disabled with the reason it is. */
  readonly play: PlayAction;
  /**
   * Whether the page draws the acquisition control **at all**.
   *
   * Absent rather than disabled for a title the library holds, which is the whole point of reading
   * the offer instead of a count: a disabled Download beside a Play that works is still a page
   * offering to download something that is already on disk.
   */
  readonly acquire: boolean;
}

/**
 * The actions a title's own page offers, from the core's offer, the build's player, and the container.
 *
 * The offer decides first, and the player is only consulted once there is something to play: a build
 * with no player that said `noPlayer` over a title nobody has would hide the one fact the user can
 * act on. Every value of the offer maps to exactly one reason, which is what makes the sentence
 * beside the control the core's answer rather than a screen's guess.
 *
 * **The container comes last because it is only meaningful once there is a file.** `externalPlayer`
 * means the webview's media stack cannot demux it, so the action names mpv as the reader and the
 * screen starts that instead of a `<video>`; the classification is what tells the two apart
 * (`ADR 0023` §5). `unknown` plays in the page, with an admission; `null` is the read not knowing,
 * which is not a reason to withhold anything.
 */
export function titlePageActions(
  offer: TitleOfferDto,
  player: boolean,
  support: LocalPlaybackDto | null,
): TitlePageActions {
  if (offer === "acquire") {
    return { play: { kind: "unavailable", reason: "noFile" }, acquire: true };
  }
  if (offer === "absent") {
    return { play: { kind: "unavailable", reason: "fileMissing" }, acquire: false };
  }
  if (offer === "arriving") {
    return { play: { kind: "unavailable", reason: "arriving" }, acquire: false };
  }
  if (!player) {
    return { play: { kind: "unavailable", reason: "noPlayer" }, acquire: false };
  }
  if (support === "externalPlayer") {
    // The build has a reader for this container: mpv. The press starts it rather than being
    // refused, and the page says which reader will open the file.
    return {
      play: { kind: "play", containerUnknown: false, reader: "external" },
      acquire: false,
    };
  }
  return {
    play: { kind: "play", containerUnknown: support === "unknown", reader: "webview" },
    acquire: false,
  };
}
