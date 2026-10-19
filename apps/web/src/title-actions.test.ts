import { describe, expect, it } from "vitest";

import { downloadAction, seasonAcquireMessage, seasonDownloadLabel } from "./title-actions.js";

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
