/**
 * What Tab may reach inside a full-window dialog.
 *
 * `packages/ui`'s `Dialog` owns this for the dialogs it draws, but the profile selector and its editor
 * are custom full-window surfaces rather than that primitive, so the rule has to travel with them. A
 * modal that lets Tab walk into the page behind it lies about `aria-modal`, and the keyboard user ends
 * up on a control they cannot see.
 */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

/** The elements a Tab press may land on, in document order, ignoring anything the browser will not paint. */
function focusableWithin(container: HTMLElement): readonly HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (element) => element.offsetParent !== null,
  );
}

/**
 * Wraps Tab at the ends of `container`.
 *
 * Called from a keydown handler on the dialog rather than from the document, so a nested dialog
 * answers its own press first. The middle of the list is left to the browser, which is what keeps the
 * platform's own Tab order and skips a control the browser has disabled.
 */
export function keepFocusInside(
  event: {
    readonly key: string;
    readonly shiftKey: boolean;
    readonly preventDefault: () => void;
  },
  container: HTMLElement | null,
): void {
  if (event.key !== "Tab" || container === null) {
    return;
  }
  const focusable = focusableWithin(container);
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (first === undefined || last === undefined) {
    event.preventDefault();
    return;
  }
  const active = document.activeElement;
  const outside = active === null || !container.contains(active);
  if (event.shiftKey ? active === first || outside : active === last || outside) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  }
}
