/**
 * The title page's download and season-acquisition copy.
 *
 * These are functions rather than JSX branches so the lifecycle-to-label mapping — including the
 * percent formatting the old page never re-read — is testable without a DOM.
 */

import type { DownloadState, SeasonAcquireResult } from "./api.js";
import { downloadLabel, downloadTone } from "./download-state.js";

/** A download button's text and whether it can be pressed. */
export interface DownloadAction {
  readonly label: string;
  readonly disabled: boolean;
}

/**
 * What a download control offers for a lifecycle state and its completion.
 *
 * A job that is running or finished cannot be started again; a failed one can. "None" takes the
 * caller's fallback so a series without episodes still says "Download series".
 */
export function downloadAction(
  state: DownloadState,
  percent: number | null,
  noneLabel = "Download",
): DownloadAction {
  const tone = downloadTone(state);
  if (tone === "none") return { label: noneLabel, disabled: false };
  if (tone === "active") {
    return { label: "Downloading " + String(percentOf(percent)) + "%", disabled: true };
  }
  if (tone === "failed") return { label: downloadLabel(state), disabled: false };
  return { label: downloadLabel(state), disabled: true };
}

/**
 * The lifecycle a control draws while a just-queued job is not yet in the title read.
 *
 * The server is the authority, but the first read after a queue can race the scheduler. A local
 * "queued" is shown until the server names a state, so a successful click never falls back to
 * "Download" and invites a duplicate.
 */
export function shownDownloadState(state: DownloadState, watching: boolean): DownloadState {
  return state ?? (watching ? "queued" : null);
}

/** The label of a season's download control. */
export function seasonDownloadLabel(season: number): string {
  return "Download season " + String(season);
}

/** What a season acquisition queued, and what it could not, as one announced sentence. */
export function seasonAcquireMessage(result: SeasonAcquireResult): string {
  const parts = ["Queued " + count(result.queued, "episode", "episodes") + "."];
  if (result.alreadyPlayable > 0) {
    parts.push(count(result.alreadyPlayable, "episode was", "episodes were") + " already on disk.");
  }
  if (result.failed > 0) {
    parts.push(count(result.failed, "episode could not", "episodes could not") + " be queued.");
  }
  return parts.join(" ");
}

/** A whole percent between 0 and 100, because a job's count and a button's width disagree at the edges. */
function percentOf(percent: number | null): number {
  return Math.max(0, Math.min(100, Math.round(percent ?? 0)));
}

function count(value: number, one: string, many: string): string {
  return String(value) + " " + (value === 1 ? one : many);
}
