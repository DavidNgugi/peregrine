import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";
import { Icon, type IconName } from "./Icon.js";

/**
 * The primary navigation column (docs/DESIGN_SYSTEM.md §6, chrome; §11 rule 3).
 *
 * The window has no browser back button and no hamburger menu, so this is where the
 * screens are reached from: one labelled `nav` landmark holding one row per screen. The
 * rows are buttons here rather than links because this build has no router yet — the shell
 * switches screens from state (PG-E7-01) and PG-E7-02 replaces that with typed routes, at
 * which point the row becomes a link and keeps every other behaviour it has.
 *
 * The column is 232 px and falls back to a 60 px icon rail when the window is narrower than
 * the `lg` breakpoint. The window's own minimum is 960 logical px
 * (`apps/desktop/src-tauri/tauri.conf.json`), so the rail is what a narrow window gets; the
 * label stays in the accessibility tree at both widths rather than being hidden, because a
 * row named by an icon alone cannot be read out.
 */
export interface SidebarProps {
  /** The landmark's accessible name; a page with two `nav` elements needs both named. */
  readonly label: string;
  readonly children: ReactNode;
  readonly className?: string;
}

export function Sidebar({ label, children, className }: SidebarProps): ReactElement {
  return (
    <nav
      aria-label={label}
      className={twMerge(
        clsx(
          "flex w-[232px] shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border-subtle bg-surface-1 p-2",
          "max-lg:w-[60px]",
        ),
        className,
      )}
    >
      {children}
    </nav>
  );
}

export interface SidebarItemProps {
  readonly icon: IconName;
  readonly label: string;
  /**
   * Whether this row is the screen on show.
   *
   * It is the selection *and* the announcement: the fill alone would be a state carried by
   * colour, which §2.2 rules out, and `aria-current="page"` is what a screen reader reads.
   */
  readonly current?: boolean;
  readonly onSelect: () => void;
  readonly className?: string;
}

/** One screen in the sidebar. */
export function SidebarItem({
  icon,
  label,
  current = false,
  onSelect,
  className,
}: SidebarItemProps): ReactElement {
  return (
    <button
      aria-current={current ? "page" : undefined}
      className={twMerge(
        clsx(
          "flex h-8 w-full items-center gap-2 rounded-md px-2 text-body",
          "transition-colors motion-reduce:transition-none",
          "focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus-ring",
          current
            ? "bg-surface-2 text-text-primary"
            : "text-text-secondary hover:bg-surface-2 hover:text-text-primary",
          "max-lg:justify-center max-lg:px-0",
        ),
        className,
      )}
      onClick={onSelect}
      type="button"
    >
      <Icon name={icon} size={20} />
      {/* `sr-only` rather than `hidden` at rail width: it keeps the row's name. */}
      <span className="truncate max-lg:sr-only">{label}</span>
    </button>
  );
}
