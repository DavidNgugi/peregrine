import type { NextUp, PostPlayState } from "@peregrine/player-ui";
import { createRef } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { PostPlayPanel, TracksPanel } from "./Player.js";

const NEXT_UP: NextUp = {
  titleId: 7,
  seasonNumber: 2,
  episodeNumber: 3,
  episodeId: 42,
  name: "The Long Way Down",
};

function panel(state: PostPlayState): string {
  return renderToStaticMarkup(
    <PostPlayPanel
      onCancel={() => {}}
      onPlayNow={() => {}}
      onReplay={() => {}}
      panelRef={createRef<HTMLElement>()}
      state={state}
    />,
  );
}

describe("PostPlayPanel", () => {
  it("draws the countdown, the offered episode, and both choices", () => {
    const html = panel({
      phase: "countdown",
      remainingMs: 10_000,
      nextUp: NEXT_UP,
      prefetch: "idle",
    });
    expect(html).toContain("Up next");
    expect(html).toContain("S2 E3");
    expect(html).toContain("The Long Way Down");
    expect(html).toContain("Playing in 10s");
    expect(html).toContain("Play now");
    expect(html).toContain("Cancel");
    expect(html).toContain('data-autofocus="true"');
  });

  it("counts down to the last second without showing a negative", () => {
    expect(
      panel({ phase: "countdown", remainingMs: 1, nextUp: NEXT_UP, prefetch: "idle" }),
    ).toContain("Playing in 1s");
  });

  it("draws Replay on the ended panel and never a Play now", () => {
    const html = panel({ phase: "ended", remainingMs: 0, nextUp: null, prefetch: "idle" });
    expect(html).toContain("Playback finished");
    expect(html).toContain("Replay");
    expect(html).not.toContain("Play now");
    expect(html).not.toContain("Cancel");
  });

  it("draws nothing while the coordinator is hidden", () => {
    expect(panel({ phase: "hidden", remainingMs: 0, nextUp: NEXT_UP, prefetch: "idle" })).toBe("");
  });
});

const TRACK = {
  id: 4,
  language: "en",
  label: "English",
  format: "srt",
  forced: false,
  sdh: true,
  default: true,
  url: "/subs/4",
};

function sheet(props: Partial<Parameters<typeof TracksPanel>[0]> = {}): string {
  return renderToStaticMarkup(
    <TracksPanel
      audio={null}
      audioLanguage={null}
      onAudio={() => {}}
      onClose={() => {}}
      onSubtitle={() => {}}
      selectedAudio={null}
      selectedSubtitle={null}
      subtitles={[]}
      {...props}
    />,
  );
}

describe("TracksPanel", () => {
  it("says so when a file has no text subtitles, with Off still chosen", () => {
    const html = sheet();
    expect(html).toContain("This file has no text subtitles.");
    expect(html).toMatch(/aria-checked="true"[^>]*>.*Off/);
    expect(html).toContain("One audio track");
  });

  it("marks the chosen track and names its flags", () => {
    const html = sheet({ subtitles: [TRACK], selectedSubtitle: 0 });
    expect(html).toContain("English (SDH)");
    expect(html).not.toContain("no text subtitles");
    expect(html.match(/aria-checked="true"/g)).toHaveLength(1);
    expect(html).toMatch(/aria-checked="true"[^>]*>.*English/);
  });

  it("lists audio renditions when there is more than one, and the host's language otherwise", () => {
    const html = sheet({
      audio: {
        source: "hls",
        options: [
          { id: "hls-0", label: "English", language: "en" },
          { id: "hls-1", label: "Japanese", language: "ja" },
        ],
      },
      selectedAudio: 1,
    });
    expect(html).toMatch(/aria-checked="true"[^>]*>.*Japanese/);
    expect(sheet({ audioLanguage: "ja" })).toContain("Audio: ja");
  });

  it("keeps spatial focus inside the sheet", () => {
    expect(sheet()).toContain('data-focus-scope="true"');
  });
});
