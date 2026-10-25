import { createI18n } from "@peregrine/i18n";
import type { IpcError, PlaybackSessionDto, SubtitleTrackDto } from "@peregrine/ipc-client";
import { initialPostPlayState, type NextUp, type PostPlayState } from "@peregrine/player-ui";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import type { Playback } from "../hooks/usePlayback.js";
import { nextUpFromSession, startNextEpisode, stepPostPlay } from "../hooks/usePlayerMarkers.js";
import { PostPlayPanel } from "./PostPlayPanel.js";
import { PlayerView } from "./PlayerScreen.js";

const i18n = createI18n();

// The copy this slice adds lands in packages/i18n with the parent's change. Pinning it here keeps
// these tests about behaviour rather than about the literal key names a missing catalogue renders.
i18n.addResourceBundle(
  "en",
  "translation",
  {
    player: {
      skip: {
        intro: "Skip Intro",
        recap: "Skip Recap",
        preview: "Skip Preview",
      },
      nextUp: {
        title: "Up next",
        countdown: "Next episode in {{seconds}}s",
        playNow: "Play now",
        cancel: "Cancel",
      },
      ended: {
        title: "The end.",
        replay: "Replay",
      },
    },
  },
  true,
  true,
);

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
    // A bound the skip control can clamp to, and the marker cache's answer for a file with none.
    durationMs: 1_440_000,
    markers: [],
    nextEpisode: null,
    audioLanguage: "en",
    ...overrides,
  };
}

/** One marker as the session carries it, with only the field under test varying. */
function marker(
  overrides: Partial<PlaybackSessionDto["markers"][number]> = {},
): PlaybackSessionDto["markers"][number] {
  return {
    kind: "intro",
    startMs: 0,
    endMs: 90_000,
    source: "chapters",
    confidence: 90,
    ...overrides,
  };
}

/** The episode the core offers after this one. */
const NEXT_EPISODE: NextUp = {
  titleId: 7,
  seasonNumber: 2,
  episodeNumber: 3,
  episodeId: 99,
  name: "The Long Way Down",
};

/** The countdown a credits marker opens when the session offered a follow-up. */
function countdownState(): PostPlayState {
  const opened = session({
    markers: [marker({ kind: "credits", startMs: 1_320_000, endMs: null })],
    nextEpisode: {
      seasonNumber: NEXT_EPISODE.seasonNumber,
      episodeNumber: NEXT_EPISODE.episodeNumber,
      episodeId: NEXT_EPISODE.episodeId,
      name: NEXT_EPISODE.name,
    },
  });
  return stepPostPlay(
    { ...initialPostPlayState(), nextUp: nextUpFromSession(opened) },
    { type: "credits" },
  ).state;
}

function postPlayMarkup(state: PostPlayState): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PostPlayPanel
        onCancel={() => undefined}
        onClose={() => undefined}
        onPlayNow={() => undefined}
        onReplay={() => undefined}
        state={state}
      />
    </I18nextProvider>,
  );
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

describe("PlayerView skip markers", () => {
  it("draws the skip control for the intro window the position is inside", () => {
    const html = markup({ session: session({ markers: [marker()] }) });

    expect(html).toContain("Skip Intro");
    expect(html).toContain('data-player-skip="intro"');
  });

  it("draws no skip control for a session with no markers", () => {
    const html = markup({ session: session() });

    expect(html).not.toContain("Skip Intro");
    expect(html).not.toContain("data-player-skip");
  });

  it("does not offer to skip the credits", () => {
    // Credits are the post-play surface's boundary, not a window to jump past.
    const html = markup({
      session: session({ markers: [marker({ kind: "credits", endMs: null })] }),
    });

    expect(html).not.toContain("data-player-skip");
    expect(html).not.toContain("Skip Intro");
  });

  it("leaves the skip and post-play controls off the external player's surface", () => {
    // mpv owns the picture and its own keyboard handling, so a control here would act on nothing.
    const html = markup({
      session: session({ support: "externalPlayer", markers: [marker()] }),
    });

    expect(html).not.toContain("Skip Intro");
    expect(html).not.toContain("data-player-skip");
    expect(html).not.toContain("data-player-post-play");
  });
});

describe("post-play panel", () => {
  // The static harness has no DOM or event loop, so Cancel and Play now are exercised through the
  // same transitions their click handlers call, with the rendered card proving both controls exist.
  it("draws the countdown and both choices when a credits marker offers a next episode", () => {
    const state = countdownState();

    expect(state.phase).toBe("countdown");
    const html = postPlayMarkup(state);
    expect(html).toContain("Next episode in 10s");
    expect(html).toContain("Play now");
    expect(html).toContain("Cancel");
    expect(html).toContain("The Long Way Down");
  });

  it("hides the countdown on Cancel without starting anything", () => {
    const cancelled = stepPostPlay(countdownState(), { type: "cancel" });

    expect(cancelled.state.phase).toBe("hidden");
    expect(cancelled.start).toBeNull();
    expect(postPlayMarkup(cancelled.state)).toBe("");
  });

  it("starts the offered episode exactly once when Play now is chosen", () => {
    const open = vi.fn(() => Promise.resolve());
    const chosen = stepPostPlay(countdownState(), { type: "start-next" });

    expect(chosen.start).toEqual(NEXT_EPISODE);
    if (chosen.start !== null) {
      startNextEpisode(chosen.start, open);
    }
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith(7, 2, 3);
  });

  it("still opens the offer when the prefetch failed", () => {
    // A cache miss is not a refusal: the same normal open serves the episode.
    const open = vi.fn(() => Promise.resolve());
    const chosen = stepPostPlay(
      { ...countdownState(), prefetch: "failed" },
      { type: "start-next" },
    );

    expect(chosen.start).toEqual(NEXT_EPISODE);
    if (chosen.start !== null) {
      startNextEpisode(chosen.start, open);
    }
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("draws Replay and the way back when the media ended with no next episode", () => {
    const ended = stepPostPlay(initialPostPlayState(), { type: "ended" });

    expect(ended.state.phase).toBe("ended");
    const html = postPlayMarkup(ended.state);
    expect(html).toContain("The end.");
    expect(html).toContain("Replay");
    expect(html).toContain("Back to the title");
  });

  it("draws the ended panel rather than a dead countdown when the offer is missing", () => {
    const state: PostPlayState = {
      ...initialPostPlayState(),
      phase: "countdown",
      remainingMs: 10_000,
      nextUp: null,
    };

    expect(postPlayMarkup(state)).toContain("Replay");
  });

  it("draws nothing before a boundary", () => {
    expect(postPlayMarkup(initialPostPlayState())).toBe("");
  });
});
