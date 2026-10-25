import type { NextUp, PostPlayState } from "@peregrine/player-ui";
import { createRef } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { PostPlayPanel } from "./Player.js";

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
