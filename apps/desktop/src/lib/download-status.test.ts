import { describe, expect, it } from "vitest";
import {
  downloadMarks,
  downloadPercentOf,
  downloadProgressMarks,
  downloadStatusLabelKey,
  downloadStatusOf,
  downloadsFirst,
  rankByDownload,
} from "./download-status.js";

describe("downloadStatusOf", () => {
  it("groups every state the machine can store", () => {
    // The vocabulary is docs/FLOWS.md SM1's; a state added there with no home here would draw
    // nothing, which is a silent omission rather than a compile error, so each one is named.
    for (const state of ["queued", "inspecting", "metadata", "downloading", "paused"]) {
      expect(downloadStatusOf(state), state).toBe("active");
    }
    for (const state of ["seeding", "completed"]) {
      expect(downloadStatusOf(state), state).toBe("done");
    }
    for (const state of ["failed", "blocked"]) {
      expect(downloadStatusOf(state), state).toBe("failed");
    }
  });

  it("says nothing for a state that is no longer a download", () => {
    expect(downloadStatusOf("removed")).toBeNull();
  });

  it("says nothing for a spelling it does not know", () => {
    // The core is the only authority on a state; an unknown string is an absence of a signal and
    // not an invented one.
    expect(downloadStatusOf("future-state")).toBeNull();
    expect(downloadStatusOf("")).toBeNull();
  });

  it("names a word so the colour is never the only signal", () => {
    expect(downloadStatusLabelKey("active")).toBe("downloads.status.active");
    expect(downloadStatusLabelKey("done")).toBe("downloads.status.done");
    expect(downloadStatusLabelKey("failed")).toBe("downloads.status.failed");
  });
});

describe("downloadPercentOf", () => {
  it("stays below 100 until the whole payload has arrived", () => {
    // 999 permille is a piece short of a complete torrent; a rounded-up 100 would claim a
    // download the queue is still waiting on.
    expect(downloadPercentOf(999)).toBe(99);
    expect(downloadPercentOf(1_000)).toBe(100);
  });

  it("clamps to the range the core documents", () => {
    expect(downloadPercentOf(0)).toBe(0);
    expect(downloadPercentOf(-5)).toBe(0);
    expect(downloadPercentOf(1_200)).toBe(100);
  });
});

describe("downloadProgressMarks", () => {
  it("keeps a nearly finished transfer at 99", () => {
    const marks = downloadProgressMarks([
      { titleId: 7, state: "downloading", progressPermille: 999 },
    ]);
    expect(marks.get(7)).toBe(99);
  });

  it("draws nothing for a job that is no longer active", () => {
    const marks = downloadProgressMarks([
      { titleId: 7, state: "completed", progressPermille: 1_000 },
    ]);
    expect(marks.size).toBe(0);
  });
});

describe("downloadMarks", () => {
  it("is empty for an empty queue", () => {
    expect(downloadMarks([]).size).toBe(0);
  });

  it("keeps one status per title", () => {
    const marks = downloadMarks([{ titleId: 7, state: "downloading" }]);
    expect(marks.get(7)).toBe("active");
  });

  it("prefers the most urgent job when a title has several", () => {
    // A retry in flight must not wear its predecessor's failure, and a finished copy must not
    // hide a transfer still running.
    expect(
      downloadMarks([
        { titleId: 7, state: "failed" },
        { titleId: 7, state: "downloading" },
      ]).get(7),
    ).toBe("active");
    expect(
      downloadMarks([
        { titleId: 7, state: "completed" },
        { titleId: 7, state: "blocked" },
      ]).get(7),
    ).toBe("failed");
    expect(
      downloadMarks([
        { titleId: 7, state: "seeding" },
        { titleId: 7, state: "completed" },
      ]).get(7),
    ).toBe("done");
  });

  it("carries no mark for a removed job", () => {
    expect(downloadMarks([{ titleId: 7, state: "removed" }]).has(7)).toBe(false);
  });
});

describe("rankByDownload", () => {
  const items = [
    { titleId: 1, name: "one" },
    { titleId: 2, name: "two" },
    { titleId: 3, name: "three" },
  ];

  it("brings an active transfer to the front and keeps the rest in order", () => {
    const ranked = rankByDownload(items, downloadMarks([{ titleId: 3, state: "downloading" }]));
    expect(ranked.map((item) => item.titleId)).toEqual([3, 1, 2]);
  });

  it("puts a completed download above the untouched titles", () => {
    const done = rankByDownload(items, downloadMarks([{ titleId: 2, state: "completed" }]));
    expect(done.map((item) => item.titleId)).toEqual([2, 1, 3]);
  });

  it("puts a running transfer above a completed one", () => {
    // The two groups are ordered, not merged: what is still arriving outranks what has arrived.
    const ranked = rankByDownload(
      items,
      downloadMarks([
        { titleId: 3, state: "downloading" },
        { titleId: 2, state: "completed" },
      ]),
    );
    expect(ranked.map((item) => item.titleId)).toEqual([3, 2, 1]);
  });

  it("does not hoist a failed job", () => {
    // A failure already wears the red mark; ranking by alarm would put it over an untouched title.
    const failed = rankByDownload(items, downloadMarks([{ titleId: 2, state: "failed" }]));
    expect(failed).toBe(items);
  });

  it("returns the same array when nothing would move", () => {
    // A new array identity would re-key a virtualized grid on every read.
    expect(rankByDownload(items, downloadMarks([]))).toBe(items);
  });

  it("handles the empty list", () => {
    expect(rankByDownload([], downloadMarks([{ titleId: 1, state: "downloading" }]))).toEqual([]);
  });
});

describe("downloadsFirst", () => {
  const items = [
    { titleId: 1, name: "one" },
    { titleId: 2, name: "two" },
  ];
  const make = (row: { readonly title: string | null; readonly titleId: number }) => ({
    titleId: row.titleId,
    name: row.title ?? "",
  });

  it("puts a download the provider did not send above the feed", () => {
    const merged = downloadsFirst(
      items,
      [
        {
          titleId: 9,
          title: "nine",
          titleKind: "movie",
          poster: null,
          state: "completed",
          hasMediaFile: false,
        },
      ],
      "movie",
      make,
    );
    expect(merged.map((item) => item.titleId)).toEqual([9, 1, 2]);
  });

  it("puts an in-progress transfer above a completed one", () => {
    const merged = downloadsFirst(
      items,
      [
        { titleId: 9, title: "nine", titleKind: "movie", poster: null, state: "completed", hasMediaFile: false },
        { titleId: 8, title: "eight", titleKind: "movie", poster: null, state: "downloading", hasMediaFile: true },
      ],
      "movie",
      make,
    );
    expect(merged.map((item) => item.titleId)).toEqual([8, 9, 1, 2]);
  });

  it("keeps the provider row for a title that is both and does not draw it twice", () => {
    const merged = downloadsFirst(
      items,
      [{ titleId: 2, title: "two", titleKind: "movie", poster: null, state: "downloading", hasMediaFile: true }],
      "movie",
      make,
    );
    expect(merged.map((item) => item.titleId)).toEqual([2, 1]);
    expect(merged[0]).toBe(items[1]);
  });

  it("leaves the other kind and a failed job where they are", () => {
    const merged = downloadsFirst(
      items,
      [
        { titleId: 9, title: "nine", titleKind: "show", poster: null, state: "completed", hasMediaFile: false },
        { titleId: 8, title: "eight", titleKind: "movie", poster: null, state: "failed", hasMediaFile: false },
      ],
      "movie",
      make,
    );
    expect(merged).toBe(items);
  });
});
