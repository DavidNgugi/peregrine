/**
 * What a download's state means for a title a person is looking at.
 *
 * **The classification is the screen's vocabulary, not the domain's.** The state machine has ten
 * states because it has ten edges to police (docs/FLOWS.md SM1); a card needs to say one of three
 * things — "it is coming", "it is here", "it did not work" — and a fourth, quieter answer of
 * "nothing to say". Mapping the ten onto the three here, once, is what keeps a browse tile, a
 * library tile, and a queue row from each inventing their own idea of what `seeding` looks like.
 *
 * **It is a projection and not a rule.** Nothing here decides whether a download may start or what
 * state follows which: those are `peregrine-domain`'s and arrive over IPC. This reads a stored
 * spelling and names the tone a person sees, the same way the queue screen already grouped states
 * into finished and terminal. The core is still the only thing that decides a job's state.
 */

/** What a title's download is doing, as a card says it. */
export type DownloadStatus = "active" | "done" | "failed";

/** The subset of a download row this module reads. Structural, so a DTO is not required to test it. */
export interface DownloadMarkSource {
  readonly titleId: number;
  readonly state: string;
}

/**
 * What one stored state looks like on a card, or `null` for a state with nothing to say.
 *
 * `removed` is deliberately silent: a job the user cancelled is not in the download any more, and
 * a red banner for a choice someone made is an accusation rather than information. `blocked` is
 * red because the gate refused it and a person needs to know why their request did nothing.
 */
export function downloadStatusOf(state: string): DownloadStatus | null {
  switch (state) {
    case "queued":
    case "inspecting":
    case "metadata":
    case "downloading":
    case "paused":
      return "active";
    case "seeding":
    case "completed":
      return "done";
    case "failed":
    case "blocked":
      return "failed";
    default:
      return null;
  }
}

/** The colour a status draws, from most urgent to least, for picking one of several jobs. */
const URGENCY: Readonly<Record<DownloadStatus, number>> = { active: 0, failed: 1, done: 2 };

/**
 * One status per title, preferring the one that matters most.
 *
 * A title can have more than one job — a failed attempt beside a retry, or a finished film still
 * seeding while an upgrade runs — and the card has room for one mark. The most urgent wins:
 * `active` over `failed` over `done`, so a retry in flight does not wear its predecessor's
 * failure and a completion does not hide a transfer that is still running.
 */
export function downloadMarks(
  downloads: readonly DownloadMarkSource[],
): ReadonlyMap<number, DownloadStatus> {
  const marks = new Map<number, DownloadStatus>();
  for (const job of downloads) {
    const status = downloadStatusOf(job.state);
    if (status === null) {
      continue;
    }
    const held = marks.get(job.titleId);
    if (held === undefined || URGENCY[status] < URGENCY[held]) {
      marks.set(job.titleId, status);
    }
  }
  return marks;
}

/**
 * The same items, with the titles whose download is running brought to the front.
 *
 * **A stable partition rather than a sort**, so the provider's own order survives inside each half:
 * "popular first" is still the answer it was, and the transfer the user came back for is at the top
 * of the page rather than somewhere on it. Nothing is dropped and nothing is ranked by anything
 * else — an item with no mark and an item whose download is finished both stay in provider order.
 *
 * The original array is returned untouched when the split would change nothing, so a Browse grid
 * does not re-key its window on every read.
 */
export function rankByDownload<T extends { readonly titleId: number }>(
  items: readonly T[],
  marks: ReadonlyMap<number, DownloadStatus>,
): readonly T[] {
  const active: T[] = [];
  const rest: T[] = [];
  for (const item of items) {
    if (marks.get(item.titleId) === "active") {
      active.push(item);
    } else {
      rest.push(item);
    }
  }
  return active.length === 0 || rest.length === 0 ? items : [...active, ...rest];
}

/** The catalogue key for a status's word, which is how the colour is never carried alone. */
export function downloadStatusLabelKey(status: DownloadStatus): string {
  return `downloads.status.${status}`;
}

/** One status per title, which is what a screen hands every card it draws. */
export type DownloadMarks = ReadonlyMap<number, DownloadStatus>;

/**
 * The two props a card needs to draw a title's download mark, or none.
 *
 * A helper rather than four lines copied into every card site: the mapping from a status to its
 * localized word is one rule, and a screen that built the label itself would be a second place the
 * key is spelled.
 */
export function downloadCardProps(
  marks: DownloadMarks,
  titleId: number,
  t: (key: string) => string,
): { readonly downloadStatus?: DownloadStatus; readonly downloadLabel?: string } {
  const status = marks.get(titleId);
  return status === undefined
    ? {}
    : { downloadStatus: status, downloadLabel: t(downloadStatusLabelKey(status)) };
}

