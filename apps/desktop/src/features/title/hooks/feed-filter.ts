/**
 * The feed's opened-title filter, as a pure function.
 *
 * Separate from the hook for the reason `rails-follow.ts` and `refresh-follow.ts` are separate
 * from theirs: the Node test environment has no DOM, so an effect that filters cannot be observed.
 * The rule can be held here: no entry point of the feed — the recommendation head the core
 * answered, or a later page of a genre the recommendations share — may draw the title whose page
 * the feed is on.
 *
 * **A browse page cannot know which page asked for it.** The core drops the opened title from the
 * recommendation head, but the feed continues with catalogue pages read by genre, and those pages
 * are the same for every title in the genre. Filtering after the merge is what makes the rule true
 * of every source rather than only of the one that knows which page is open.
 */

import type { RailItemDto } from "@peregrine/ipc-client";

/**
 * `items` without the title whose page is open.
 *
 * `null` means no title route is open, which the shell is on every other screen: nothing is
 * filtered rather than everything, because there is no page to keep out of its own feed.
 */
export function withoutTitle(
  items: readonly RailItemDto[],
  titleId: number | null,
): readonly RailItemDto[] {
  return titleId === null ? items : items.filter((item) => item.titleId !== titleId);
}
