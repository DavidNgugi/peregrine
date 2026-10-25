/**
 * Where a list screen was when it was left.
 *
 * A spatial remote has no pointer to re-find a card: without this, Back from a title lands at the
 * top of a long rail and the person has to walk the grid again. The memory is keyed by the URL the
 * screen owns, and stores the page scroll plus the focused control's stable key.
 */

export interface RouteMemory {
  readonly scrollY: number;
  readonly focusKey: string | null;
}

const memories = new Map<string, RouteMemory>();
const LIMIT = 24;

/** Remembers one screen, evicting the oldest entry when the map is full. */
export function rememberRoute(key: string, memory: RouteMemory): void {
  if (key === "") return;
  memories.delete(key);
  memories.set(key, memory);
  if (memories.size > LIMIT) {
    const oldest = memories.keys().next().value;
    if (oldest !== undefined) memories.delete(oldest);
  }
}

/** The last state for a screen, or null when it has not been left yet. */
export function recallRoute(key: string): RouteMemory | null {
  return memories.get(key) ?? null;
}

/** A stable key for a focusable control. Explicit keys beat a link's href, which beats its label. */
export function focusKeyOf(element: HTMLElement): string {
  if (element.dataset.focusKey !== undefined) return element.dataset.focusKey;
  const link = element.closest<HTMLAnchorElement>("a[href]");
  if (link !== null) return "href:" + String(link.getAttribute("href"));
  const label = element.getAttribute("aria-label") ?? element.textContent ?? "";
  return "label:" + label.trim().replace(/\s+/gu, " ").slice(0, 120);
}

/** Focuses the remembered control and brings it back into view, or leaves focus alone. */
export function restoreFocus(key: string): boolean {
  const target = Array.from(document.querySelectorAll<HTMLElement>("[data-focus]")).find(
    (element) => element.offsetParent !== null && focusKeyOf(element) === key,
  );
  if (target === undefined) return false;
  target.focus({ preventScroll: true });
  target.scrollIntoView({ block: "nearest", inline: "nearest" });
  return true;
}
