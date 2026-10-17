import { describe, expect, it } from "vitest";

import { heroPlay, titlePageActions } from "./play-action.js";

describe("heroPlay", () => {
  it("plays only when there is a file and something that can play it", () => {
    expect(heroPlay(true, true)).toEqual({
      kind: "play",
      containerUnknown: false,
      reader: "webview",
    });
  });

  it("names the title's own reason first, because that is the one a person can act on", () => {
    // Both facts are false and the answer is still `noFile`: a reader can acquire a file, and nobody
    // can install a player this build does not have.
    expect(heroPlay(false, true)).toEqual({ kind: "unavailable", reason: "noFile" });
    expect(heroPlay(false, false)).toEqual({ kind: "unavailable", reason: "noFile" });
  });

  it("says when the build has no player rather than drawing a control that fires nothing", () => {
    // This build's state: `PG-E16` owns playback and no surface for it exists, so a screen is handed no
    // handler and Play is disabled with a reason — never enabled and inert.
    expect(heroPlay(true, false)).toEqual({ kind: "unavailable", reason: "noPlayer" });
  });
});

describe("titlePageActions", () => {
  it("offers Play and no acquisition for a title the library holds", () => {
    // **The rule a Download control must not get wrong.** A copy is on disk, so Play is the action and
    // Download is not drawn at all — neither disabled nor enabled, because either one is a page
    // offering to fetch what the user already has.
    expect(titlePageActions("play", true, null)).toEqual({
      play: { kind: "play", containerUnknown: false, reader: "webview" },
      acquire: false,
    });
  });

  it("still says why Play cannot be taken when the build has no player", () => {
    // The copy exists and the button is inert for the build's reason, which is a different sentence
    // from "there is nothing here" — and the acquisition control stays absent either way.
    expect(titlePageActions("play", false, null)).toEqual({
      play: { kind: "unavailable", reason: "noPlayer" },
      acquire: false,
    });
  });

  it("offers acquisition only when nothing is held and nothing is arriving", () => {
    expect(titlePageActions("acquire", true, null)).toEqual({
      play: { kind: "unavailable", reason: "noFile" },
      acquire: true,
    });
    // A build with no player does not change what the core said about the title: the fact a reader can
    // act on is still that there is no file, which is why the offer is answered first.
    expect(titlePageActions("acquire", false, null)).toEqual({
      play: { kind: "unavailable", reason: "noFile" },
      acquire: true,
    });
  });

  it("distinguishes a file that is gone from a file that never was", () => {
    // `absent` is a title the library has a row for and cannot see the bytes of — an unplugged drive,
    // a file moved behind the app. Saying "the library holds no file for this title yet" there would be
    // telling the user something the core did not say, and the acquisition control stays absent because
    // asking again would record a second copy of a file the library already knows about.
    expect(titlePageActions("absent", true, null)).toEqual({
      play: { kind: "unavailable", reason: "fileMissing" },
      acquire: false,
    });
  });

  it("says a transfer is running rather than offering a second one", () => {
    expect(titlePageActions("arriving", true, null)).toEqual({
      play: { kind: "unavailable", reason: "arriving" },
      acquire: false,
    });
  });

  it("routes a container the page cannot read to the external player", () => {
    // An MKV is the tenth file in this project's own library: HEVC inside Matroska, which a WebKit
    // `<video>` cannot demux (ADR 0023 §5). The reader is mpv, and the action names it rather than
    // leaving the control inert.
    expect(titlePageActions("play", true, "externalPlayer")).toEqual({
      play: { kind: "play", containerUnknown: false, reader: "external" },
      acquire: false,
    });
    // And Play still wins over the container: a build that could not play anything says so first.
    expect(titlePageActions("play", false, "externalPlayer")).toEqual({
      play: { kind: "unavailable", reason: "noPlayer" },
      acquire: false,
    });
  });

  it("plays a container it cannot name, and admits it", () => {
    // The decoder is the platform's and it is not limited to the formats Peregrine can name, so
    // withholding this would withhold playback that may well work. Saying nothing would make a failure
    // look like a bug, so the action is allowed *with* the doubt attached.
    expect(titlePageActions("play", true, "unknown")).toEqual({
      play: { kind: "play", containerUnknown: true, reader: "webview" },
      acquire: false,
    });
    expect(titlePageActions("play", true, "webview")).toEqual({
      play: { kind: "play", containerUnknown: false, reader: "webview" },
      acquire: false,
    });
    // `null` is the read not knowing which file it meant rather than a file this build cannot name,
    // and it is not a reason to make a reader doubt a Play the core offered.
    expect(titlePageActions("play", true, null)).toEqual({
      play: { kind: "play", containerUnknown: false, reader: "webview" },
      acquire: false,
    });
  });

  it("never draws an enabled control this build cannot honour", () => {
    // Every offer, player, and container: the only enabled action is Play, and only when a copy is on
    // disk and something can open it — the page's own element, mpv, or a reader this build cannot
    // name but will try (PG-E1-07).
    for (const offer of ["play", "arriving", "absent", "acquire"] as const) {
      for (const player of [true, false]) {
        for (const support of ["webview", "externalPlayer", "unknown", null] as const) {
          const actions = titlePageActions(offer, player, support);
          const playable = offer === "play" && player;
          expect(
            actions.play.kind === "play",
            `${offer}/${String(player)}/${String(support)}`,
          ).toBe(playable);
          expect(actions.acquire).toBe(offer === "acquire");
        }
      }
    }
  });
});
