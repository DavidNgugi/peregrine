/**
 * Which overlay is on top, and therefore which one answers a key.
 *
 * A dialog is never alone. A confirmation opens over a screen, and something can open over the
 * confirmation — a picker, a nested confirmation, a second dialog from a menu. When Escape is pressed
 * with two of them open, exactly one may close, and it is the one the user is looking at. Closing the
 * wrong one is not a small bug: the user sees the thing they were working in vanish from underneath
 * the thing they were actually looking at.
 *
 * That rule is here, in a module with no DOM in it, for the reason the rest of the kit is shaped this
 * way: the *rule* is what can be wrong, and it is testable without a browser. The component that
 * renders a dialog registers itself on mount, asks `isTop` when a key arrives, and unregisters on
 * unmount; that wiring is three lines and no decisions.
 *
 * The stack is a factory rather than module-level state so a test can have one of its own, with a
 * default instance for the application, which has exactly one stack because it has exactly one user.
 */

/** The overlays currently open, oldest first. */
export interface OverlayStack {
  /** Registers an overlay as open, and answers whether it is now the top one. */
  readonly push: (id: string) => boolean;
  /** Registers an overlay as closed. Removing one that is not open changes nothing. */
  readonly remove: (id: string) => void;
  /** Whether `id` is the overlay a key press belongs to. */
  readonly isTop: (id: string) => boolean;
  /** The overlay on top, or `null` when nothing is open. */
  readonly top: () => string | null;
  /** How many overlays are open. */
  readonly size: () => number;
}

/** Builds a stack. Tests want their own; the application wants the one below. */
export function createOverlayStack(): OverlayStack {
  const open: string[] = [];
  return {
    push: (id) => {
      // Idempotent: a component that re-renders does not stack itself twice, and a double push would
      // make Escape need pressing twice for one dialog.
      const at = open.indexOf(id);
      if (at !== -1) {
        open.splice(at, 1);
      }
      open.push(id);
      return open[open.length - 1] === id;
    },
    remove: (id) => {
      const at = open.indexOf(id);
      if (at !== -1) {
        open.splice(at, 1);
      }
    },
    isTop: (id) => open[open.length - 1] === id,
    top: () => open[open.length - 1] ?? null,
    size: () => open.length,
  };
}

/**
 * The application's stack.
 *
 * One, because there is one window and one user: a second stack would be a second answer to "what is
 * on top", which is the question this module exists to answer once.
 */
export const overlays: OverlayStack = createOverlayStack();
