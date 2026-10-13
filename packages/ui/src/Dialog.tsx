import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { useEffect, useId, useRef } from "react";
import { IconButton } from "./IconButton.js";
import { twMerge } from "./merge.js";
import { overlays, type OverlayStack } from "./overlay-stack.js";

/**
 * A surface above the page that holds one decision.
 *
 * The DOM half of `overlay-stack.ts`, and deliberately thin: the rule about *which* dialog answers a
 * key lives in that module, where it is testable without a browser, and this component only registers
 * itself, asks, and unregisters. What is left here is markup and two pieces of focus behaviour that
 * cannot be expressed anywhere else:
 *
 * - **Focus moves in when the dialog opens** and returns to whatever had it when the dialog closes.
 *   A dialog that leaves focus behind the overlay is unreachable by keyboard: the user tabs through
 *   the page underneath while the dialog sits on top of it.
 * - **Escape closes it, and only the top one.** The decision is the stack's; the listener is here
 *   because a key press arrives at the document.
 *
 * `aria-modal` and `role="dialog"` are set together on purpose: `aria-modal` alone is not a dialog,
 * and `role="dialog"` alone does not tell a screen reader that the rest of the page is inert while it
 * is open. The label is the heading this component draws, wired by `aria-labelledby` to an id it
 * mints — a dialog whose title is only visually associated is an unlabelled dialog.
 */
export interface DialogProps {
  /** The dialog's title, which is also what a screen reader announces when it opens. */
  readonly title: string;
  /** A sentence under the title, associated the same way. */
  readonly description?: string;
  readonly children: ReactNode;
  /** What to do when the user closes it: Escape, the close control, or the backdrop. */
  readonly onClose: () => void;
  /**
   * The accessible name of the close control.
   *
   * The caller's, not the kit's: the kit holds no copy (the i18n package does), and a close button
   * labelled with the dialog's title would announce the wrong thing.
   */
  readonly closeLabel: string;
  /** How wide it may grow. `md` is the default: a dialog is a decision, not a screen. */
  readonly size?: "sm" | "md";
  /** Which stack this dialog belongs to. Tests pass their own; the application uses the one. */
  readonly stack?: OverlayStack;
  readonly className?: string;
}

const SIZES: Readonly<Record<NonNullable<DialogProps["size"]>, string>> = {
  sm: "max-w-sm",
  md: "max-w-md",
};

/**
 * Focuses the first thing inside the dialog and returns focus where it was on close.
 *
 * A separate hook because the two halves are one behaviour: what moves in has to be what is put back,
 * and splitting them across the component is how a dialog ends up restoring focus to its own close
 * button.
 */
function useFocusRestore(container: React.RefObject<HTMLElement | null>): void {
  const previous = useRef<Element | null>(null);
  useEffect(() => {
    // Effects do not run while a component is rendered to a string, which is how the kit's tests and
    // the gallery render, so neither of these lines needs a `document` guard to be testable.
    previous.current = document.activeElement;
    const first = container.current?.querySelector<HTMLElement>(
      "button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])",
    );
    first?.focus();
    return () => {
      const back = previous.current;
      previous.current = null;
      if (back instanceof HTMLElement) {
        back.focus();
      }
    };
  }, [container]);
}

export function Dialog({
  title,
  description,
  children,
  onClose,
  closeLabel,
  size = "md",
  stack = overlays,
  className,
}: DialogProps): ReactElement {
  const titleId = useId();
  const descriptionId = useId();
  const container = useRef<HTMLDivElement | null>(null);
  useFocusRestore(container);

  useEffect(() => {
    stack.push(titleId);
    const onKeyDown = (event: KeyboardEvent): void => {
      // The one decision this listener makes, and it is the stack's: a press belongs to the top
      // overlay, so a confirmation closing does not also close the dialog underneath it.
      if (event.key === "Escape" && stack.isTop(titleId)) {
        onClose();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      stack.remove(titleId);
    };
  }, [stack, titleId, onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-scrim p-4"
      // The backdrop closes on click, which is what a pointer user reaches for — and it is why
      // finding nothing: the click target is the backdrop itself rather than anything inside.
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        aria-describedby={description === undefined ? undefined : descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className={twMerge(
          clsx(
            // §4 allows elevation for what floats above the page, and a dialog is the case it names.
            "flex w-full flex-col gap-3 rounded-lg border border-border-subtle bg-surface-1 p-5 shadow-elev-3",
            SIZES[size],
          ),
          className,
        )}
        ref={container}
        role="dialog"
      >
        <header className="flex items-start justify-between gap-4">
          <h2 className="text-title-3 text-text-primary" id={titleId}>
            {title}
          </h2>
          <IconButton icon="close" label={closeLabel} onClick={onClose} size="sm" />
        </header>
        {description === undefined ? null : (
          <p className="text-body text-text-secondary" id={descriptionId}>
            {description}
          </p>
        )}
        {children}
      </div>
    </div>
  );
}
