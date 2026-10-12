import { clsx } from "clsx";
import type { ReactElement } from "react";
import { twMerge } from "./merge.js";

/**
 * An accelerator, drawn the way the platform writes it (docs/DESIGN_SYSTEM.md §9's grid,
 * §6's chrome).
 *
 * The string is passed through and never composed here: the core's shortcut registry
 * renders it from `muda`'s own `Modifiers` and `Code` for the platform the process is
 * running on (`peregrine-ipc::shortcuts`), so a sheet of glyphs in TypeScript would be a
 * second table free to disagree with the keys the shell actually dispatches on. That is
 * also why this takes `display` rather than `code`, `meta`, `ctrl`, `alt`, and `shift`: the
 * four booleans are already a rendering decision, and making it twice is how ⌘ and Ctrl
 * drift apart.
 *
 * `<kbd>` is the element for it and it is not decoration — a screen reader announces a
 * keyboard input, which a styled `span` would not.
 */
export interface ShortcutKeysProps {
  /** The accelerator in this platform's notation, exactly as the core rendered it. */
  readonly display: string;
  readonly className?: string;
}

export function ShortcutKeys({ display, className }: ShortcutKeysProps): ReactElement {
  return (
    <kbd
      className={twMerge(
        clsx(
          "shrink-0 rounded-sm border border-border-subtle bg-surface-2 px-2 py-0.5",
          "font-mono text-caption text-text-secondary",
        ),
        className,
      )}
    >
      {display}
    </kbd>
  );
}
