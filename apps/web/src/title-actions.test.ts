import { describe, expect, it } from "vitest";

import type { Episode } from "./api.js";
import {
  downloadAction,
  seasonAcquireMessage,
  seasonDownloadAction,
  seasonDownloadLabel,
  shownDownloadState,
} from "./title-actions.js";

function episode(overrides: Partial<Episode> = {}): Episode {
  return {
    seasonNumber: 1,
    episodeNumber: 1,
    name: null,
    overview: null,
    still: null,
    runtimeMinutes: null,
    rating: null,
    airDate: null,
    files: 0,
    percent: 0,
    positionMs: 0,
    watched: false,
    playable: false,
    streamable: false,
    scheduled: false,
    downloadState: null,
    downloadPercent: null,
    ...overrides,
  };
}

describe("shownDownloadState", () => {
  it("keeps a queued click visible until the title read catches up", () => {
    expect(shownDownloadState(null, true)).toBe("queued");
    expect(shownDownloadState(null, false)).toBeNull();
  });

  it("lets the server's state win once it names one", () => {
    expect(shownDownloadState("downloading", true)).toBe("downloading");
    expect(shownDownloadState("completed", true)).toBe("completed");
    expect(shownDownloadState("failed", true)).toBe("failed");
  });
});

describe("seasonDownloadAction", () => {
  it("offers the season when nothing is queued", () => {
    expect(seasonDownloadAction([episode(), episode({ episodeNumber: 2 })], 1, false)).toEqual({
      label: "Download season 1",
      disabled: false,
    });
  });

  it("counts a queued or just-clicked season as queued", () => {
    expect(seasonDownloadAction([episode({ downloadState: "queued" })], 1, false)).toEqual({
      label: "Queued season 1",
      disabled: true,
    });
    expect(seasonDownloadAction([episode()], 1, true)).toEqual({
      label: "Queued season 1",
      disabled: true,
    });
  });

  it("reports the whole season's percentage across done, active, and missing", () => {
    const action = seasonDownloadAction(
      [
        episode({ episodeNumber: 1, playable: true }),
        episode({ episodeNumber: 2, downloadState: "downloading", downloadPercent: 40 }),
      ],
      1,
      false,
    );
    expect(action).toEqual({ label: "Downloading season 1 · 70%", disabled: true });
  });

  it("says downloaded only when every episode is on disk", () => {
    expect(
      seasonDownloadAction(
        [episode({ episodeNumber: 1, playable: true }), episode({ episodeNumber: 2 })],
        1,
        false,
      ),
    ).toEqual({ label: "Download season 1", disabled: false });
    expect(
      seasonDownloadAction(
        [
          episode({ episodeNumber: 1, playable: true }),
          episode({ episodeNumber: 2, playable: true }),
        ],
        1,
        false,
      ),
    ).toEqual({ label: "Season 1 downloaded", disabled: true });
  });

  it("offers a retry after a failure", () => {
    expect(seasonDownloadAction([episode({ downloadState: "failed" })], 1, true)).toEqual({
      label: "Retry season 1",
      disabled: false,
    });
  });
});

describe("downloadAction", () => {
  it("offers the download when no job exists", () => {
    expect(downloadAction(null, null)).toEqual({ label: "Download", disabled: false });
    expect(downloadAction("", 0)).toEqual({ label: "Download", disabled: false });
    expect(downloadAction("removed", 0)).toEqual({ label: "Download", disabled: false });
  });

  it("keeps a fallback label for a series without episodes", () => {
    expect(downloadAction(null, null, "Download series").label).toBe("Download series");
    expect(downloadAction("queued", 0, "Download series").label).toBe("Queued");
  });

  it("reports a queued job as a disabled wait", () => {
    expect(downloadAction("queued", 0)).toEqual({ label: "Queued", disabled: true });
    expect(downloadAction("paused", 12)).toEqual({ label: "Queued", disabled: true });
  });

  it("reports an active job with a bounded whole percent", () => {
    expect(downloadAction("downloading", 42.6)).toEqual({
      label: "Downloading 43%",
      disabled: true,
    });
    expect(downloadAction("downloading", null)).toEqual({
      label: "Downloading 0%",
      disabled: true,
    });
    expect(downloadAction("downloading", -3).label).toBe("Downloading 0%");
    expect(downloadAction("downloading", 140).label).toBe("Downloading 100%");
  });

  it("reports a finished job as a disabled download and a failed one as a retry", () => {
    expect(downloadAction("completed", 100)).toEqual({ label: "Downloaded", disabled: true });
    expect(downloadAction("seeding", 100)).toEqual({ label: "Downloaded", disabled: true });
    expect(downloadAction("failed", null)).toEqual({ label: "Try again", disabled: false });
    expect(downloadAction("blocked", null)).toEqual({ label: "Try again", disabled: false });
  });
});

describe("seasonDownloadLabel", () => {
  it("names the season rather than a generic series download", () => {
    expect(seasonDownloadLabel(1)).toBe("Download season 1");
    expect(seasonDownloadLabel(0)).toBe("Download season 0");
  });
});

describe("seasonAcquireMessage", () => {
  it("reports what queued, what was already on disk, and what failed", () => {
    expect(seasonAcquireMessage({ queued: 3, alreadyPlayable: 2, failed: 1 })).toBe(
      "Queued 3 episodes. 2 episodes were already on disk. 1 episode could not be queued.",
    );
  });

  it("omits counts that are zero", () => {
    expect(seasonAcquireMessage({ queued: 1, alreadyPlayable: 0, failed: 0 })).toBe(
      "Queued 1 episode.",
    );
    expect(seasonAcquireMessage({ queued: 0, alreadyPlayable: 1, failed: 0 })).toBe(
      "Queued 0 episodes. 1 episode was already on disk.",
    );
  });
});
