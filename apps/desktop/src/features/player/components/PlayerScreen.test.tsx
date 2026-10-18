import { createI18n } from "@peregrine/i18n";
import type { IpcError, PlaybackSessionDto, SubtitleTrackDto } from "@peregrine/ipc-client";
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
    episodeId: null,
    mediaFileId: 42,
    support: "webview",
    // The shape the core builds: loopback, an ephemeral port, and the session token in the path.
    url: "http://127.0.0.1:51234/media/abc123abc123abc123abc123abc123abc123",
    // Off by default, because a title a provider has drawn a poster for is the ordinary case and a capture
    // there would be a decode of the film to overwrite real artwork.
    needsPoster: false,
    thumbnailUrl: null,
    // No sidecars: a file without a subtitle the library indexed is the ordinary case.
    subtitles: [],
    // No stored position: a first watch, which is the ordinary case.
    resumePositionMs: null,
    ...overrides,
  };
}

/** One sidecar as the core answers it, with only the field under test varying. */
function sidecar(overrides: Partial<SubtitleTrackDto> = {}): SubtitleTrackDto {
  return {
    id: 11,
    language: "en",
    label: null,
    format: "srt",
    forced: false,
    sdh: false,
    default: false,
    url: "http://127.0.0.1:51234/subtitle/abc123abc123abc123abc123abc123abc123/11",
    ...overrides,
  };
}

/** The screen as markup, from the state the hook would have answered with. */
function markup(state: Partial<Playback> = {}, externalFailure?: IpcError | null): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PlayerView
        externalFailure={externalFailure}
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

    expect(html).toContain(
      `src="http://127.0.0.1:51234/media/abc123abc123abc123abc123abc123abc123"`,
    );
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

  it("draws no video surface for a session the external player owns", () => {
    // The webview cannot demux this container, so the picture is mpv's. A video element here would
    // be the black rectangle that made a working film look broken.
    const html = markup({ session: session({ support: "externalPlayer" }) });

    expect(html).not.toContain("<video");
    expect(html).toContain('data-player-external=""');
    expect(html).toContain("The film is open in mpv.");
  });

  it("renders one track element per sidecar, from the URL the core answered with", () => {
    // The sidecar travels the same way the film does: a URL from the core, never a path and never a
    // blob over IPC. The element is the platform's own text-track surface.
    const html = markup({
      session: session({
        subtitles: [
          sidecar({ id: 11, language: "en", sdh: true }),
          sidecar({
            id: 12,
            language: "fr",
            label: "Français",
            format: "vtt",
            forced: true,
            url: "http://127.0.0.1:51234/subtitle/abc123abc123abc123abc123abc123abc123/12",
          }),
        ],
      }),
    });

    expect(html).toContain('kind="subtitles"');
    expect(html).toContain('srcLang="en"');
    expect(html).toContain(
      'src="http://127.0.0.1:51234/subtitle/abc123abc123abc123abc123abc123abc123/11"',
    );
    expect(html).toContain(
      'src="http://127.0.0.1:51234/subtitle/abc123abc123abc123abc123abc123abc123/12"',
    );
    expect(html).toContain("en · SDH · SRT");
    expect(html).toContain("Français · Forced · VTT");
    // The menu itself, with the way out of it.
    expect(html).toContain('data-player-subtitles=""');
    expect(html).toContain("Subtitles");
    expect(html).toContain("Off");
    // Tracks are cross-origin text, so the element has to fetch them in CORS mode.
    expect(html).toContain('crossorigin="anonymous"');
  });

  it("draws no track menu for a file with no sidecars", () => {
    const html = markup({ session: session() });

    expect(html).not.toContain("<track");
    expect(html).not.toContain('data-player-subtitles=""');
    expect(html).not.toContain("Subtitles");
  });

  it("leaves the track menu to mpv on a session the external player owns", () => {
    // The sidecars may exist, but this page cannot show one and a menu that changed nothing would be
    // a control that lies about what it does.
    const html = markup({
      session: session({ support: "externalPlayer", subtitles: [sidecar()] }),
    });

    expect(html).not.toContain('data-player-subtitles=""');
    expect(html).not.toContain("<track");
  });

  it("says why the external player could not be started", () => {
    // mpv missing is a launch failure, not a media failure; the core's sentence is the one shown.
    const html = markup(
      { session: session({ support: "externalPlayer" }) },
      { code: "INTERNAL", message: "mpv is not installed", retryable: false },
    );

    expect(html).toContain("mpv is not installed");
    expect(html).toContain("could not be played");
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
