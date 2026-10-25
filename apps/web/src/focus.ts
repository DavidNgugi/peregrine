/** The two DOM behaviours the spatial rule needs: moving focus, and keeping the focused tile in view. */

import { useEffect } from "react";

import { nextIndex, type Direction } from "./spatial.js";

/** Moves focus on an arrow key, using the boxes of every visible [data-focus] control. */
export function useSpatialFocus(active: boolean): void {
  useEffect(() => {
    if (!active) return;
    const onKey = (event: KeyboardEvent): void => {
      const direction = directionOf(event.key);
      if (direction === null) return;
      const focused = document.activeElement;
      // A focused seek bar uses left and right itself; up and down still leave it.
      if (
        focused instanceof HTMLInputElement &&
        focused.type === "range" &&
        (direction === "left" || direction === "right")
      ) {
        return;
      }
      // A select's own menu owns every arrow. Text fields own horizontal arrows so the cursor can
      // move, while vertical arrows remain available to leave a search field and reach the screen
      // controls below it.
      if (
        focused instanceof HTMLTextAreaElement ||
        focused instanceof HTMLSelectElement ||
        (focused instanceof HTMLInputElement &&
          focused.type !== "range" &&
          (direction === "left" || direction === "right"))
      ) {
        return;
      }
      const scope =
        focused instanceof HTMLElement ? focused.closest<HTMLElement>("[data-focus-scope]") : null;
      const elements = Array.from(
        (scope ?? document).querySelectorAll<HTMLElement>("[data-focus]"),
      ).filter((element) => element.offsetParent !== null && !element.hasAttribute("disabled"));
      if (elements.length === 0) return;
      const current = elements.indexOf(focused as HTMLElement);
      if (current === -1) {
        focusAndReveal(elements[0]);
        event.preventDefault();
        return;
      }
      const rects = elements.map((element) => {
        const box = element.getBoundingClientRect();
        return { x: box.x, y: box.y, width: box.width, height: box.height };
      });
      focusAndReveal(elements[nextIndex(rects, current, direction)]);
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active]);
}

/** Focuses the first control a screen placed as its entry point. */
export function useAutoFocus(key?: unknown): void {
  useEffect(() => {
    if (document.querySelector('[aria-modal="true"]') !== null) return;
    const target = document.querySelector<HTMLElement>("[data-autofocus]");
    if (target !== null) target.focus({ preventScroll: false });
  }, [key]);
}

/**
 * Focus a control and bring it into view.
 *
 * `focus()` alone scrolls the *page* but not a horizontal rail, so a D-pad press past the
 * right edge would move focus to an off-screen tile and leave the rail still. `scrollIntoView`
 * with `nearest` scrolls the rail horizontally and the page vertically, and does nothing when the
 * tile is already visible.
 */
export function focusAndReveal(element: HTMLElement | undefined): void {
  if (element === undefined) return;
  element.focus({ preventScroll: true });
  element.scrollIntoView({ block: "nearest", inline: "nearest" });
}

function directionOf(key: string): Direction | null {
  switch (key) {
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
    default:
      return null;
  }
}
