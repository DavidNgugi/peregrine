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
