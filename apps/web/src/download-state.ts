/**
 * The core's download lifecycle, as a status a button or a card can draw.
 *
 * The core keeps ten states; a screen needs three answers: something is happening, it finished,
 * or it stopped. Two readings are deliberately folded: a seeding job has finished downloading,
 * and a paused job is still a queued intent rather than a failure.
 */

export type DownloadTone = "none" | "queued" | "active" | "done" | "failed";

/** What the raw state means to a control that offers the action again. */
export function downloadTone(state: string | null | undefined): DownloadTone {
  switch (state) {
    case null:
    case undefined:
    case "":
    case "removed":
      return "none";
    case "queued":
    case "inspecting":
    case "metadata":
    case "paused":
      return "queued";
    case "downloading":
    case "extracting":
    case "repairing":
    case "importing":
      return "active";
    case "seeding":
    case "completed":
      return "done";
    case "failed":
    case "blocked":
      return "failed";
    default:
      return "queued";
  }
}

/** The label a download control uses for a state. */
export function downloadLabel(state: string | null | undefined): string {
  switch (downloadTone(state)) {
    case "none":
      return "Download";
    case "queued":
      return "Queued";
    case "active":
      return "Downloading";
    case "done":
      return "Downloaded";
    case "failed":
      return "Try again";
  }
}

/** What a poster card wears for its title's downloads: a colour, and a percent while it moves. */
export interface CardMark {
  readonly tone: "active" | "done" | "failed";
  /** Whole percent of the running transfer, or null when nothing is transferring yet. */
  readonly percent: number | null;
}

/** The subset of a download row a card mark reads. */
export interface CardMarkSource {
  readonly titleId: number;
  readonly state: string;
  readonly progressPermille: number;
}

/** The mark a card keeps when a title has several jobs: a running one, then a failure, then done. */
const URGENCY: Readonly<Record<CardMark["tone"], number>> = { active: 0, failed: 1, done: 2 };

/**
 * One mark per title, as the desktop's poster card draws it.
 *
 * A queued job is already the title's download in flight, so it reads as active without a
 * percent. The percent is floored, not rounded: a job stalled on its last piece at 999 permille
 * has not finished, and 100 would say it had.
 */
export function cardMarks(rows: readonly CardMarkSource[]): ReadonlyMap<number, CardMark> {
  const marks = new Map<number, CardMark>();
  for (const row of rows) {
    const tone = downloadTone(row.state);
    if (tone === "none") continue;
    const mark: CardMark =
      tone === "queued"
        ? { tone: "active", percent: null }
        : tone === "active"
          ? {
              tone: "active",
              percent: Math.min(100, Math.max(0, Math.floor(row.progressPermille / 10))),
            }
          : { tone, percent: null };
    const held = marks.get(row.titleId);
    if (held === undefined || URGENCY[mark.tone] < URGENCY[held.tone]) {
      marks.set(row.titleId, mark);
    }
  }
  return marks;
}
