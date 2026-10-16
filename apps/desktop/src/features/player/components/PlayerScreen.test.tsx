import { createI18n } from "@peregrine/i18n";
import type { PlaybackSessionDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { Playback } from "../hooks/usePlayback.js";
import { PlayerView } from "./PlayerScreen.js";

const i18n = createI18n();

/** One open session as the core answers with it. */
function session(overrides: Partial<PlaybackSessionDto> = {}): PlaybackSessionDto {
  return {
    // Any number: the screen draws the URL and never the id, and the id exists so a *stop* can name the
    // session it means (see `usePlayback`).
    sessionId: 1,
    titleId: 7,
    mediaFileId: 42,
    support: "webview",
    // The shape the core builds: loopback, an ephemeral port, and the session token in the path.
    url: "http://127.0.0.1:51234/media/abc123abc123abc123abc123abc123abc123",
    // Off by default, because a title a provider has drawn a poster for is the ordinary case and a capture
    // there would be a decode of the film to overwrite real artwork.
    needsPoster: false,
    thumbnailUrl: null,
    // No stored position: a first watch, which is the ordinary case.
    resumePositionMs: null,
    ...overrides,
  };
}

/** The screen as markup, from the state the hook would have answered with. */
function markup(state: Partial<Playback> = {}): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PlayerView
        onClose={() => undefined}
        playback={{
          session: null,
          starting: false,
          failed: null,
          open: () => Promise.resolve(),
          close: () => Promise.resolve(),
          ...state,
        }}
      />
    </I18nextProvider>,
  );
}

describe("PlayerView", () => {
  it("points the media element at the loopback URL the core answered with", () => {
    // The whole of the surface: the URL comes from the core, the element follows it, and nothing here
    // builds one — which is what keeps the token in exactly one place (AGENTS R11).
    const html = markup({ session: session() });

    expect(html).toContain(`src="http://127.0.0.1:51234/media/abc123abc123abc123abc123abc123abc123"`);
    expect(html).toContain("controls");
    expect(html).toContain("autoPlay");
    expect(html).toContain('data-player-surface=""');
    expect(html).toContain("Playing from your library");
  });

  it("says which reader is doing the work, and what it costs", () => {
    // A container the webview cannot name plays with the doubt attached rather than a promise, and the
    // sentence is the one the title page already used — the same fact, still true.
    expect(markup({ session: session({ support: "unknown" }) })).toContain(
      "does not recognise this file&#x27;s format",
    );
    expect(markup({ session: session({ support: "externalPlayer" }) })).toContain(
      "Playing with an external player.",
    );
  });

  it("draws a reason rather than a blank page when the core refused", () => {
    // A file that moved and a listener that would not bind are different answers, and the core's own
    // sentence is what the screen shows: a player that said nothing would look like a hang.
    const refused = markup({
      failed: {
        code: "NOT_FOUND",
        message: "not found",
        detail: "That file is no longer where the last scan saw it.",
        retryable: false,
      },
    });

    expect(refused).toContain("That file is no longer where the last scan saw it.");
    expect(refused).toContain("could not be played");
    expect(refused).not.toContain("<video");
  });

  it("says it is opening the file rather than showing an empty player", () => {
    const starting = markup({ starting: true });

    expect(starting).toContain("Opening the file");
    expect(starting).not.toContain("<video");
  });

  it("offers a way back in every state", () => {
    // The route is a place, so leaving it is not something a reader should have to find another way to
    // do — including while a file is being opened and after a refusal.
    for (const state of [{}, { starting: true }, { session: session() }] as Partial<Playback>[]) {
      expect(markup(state)).toContain("Back to the title");
    }
  });
});
