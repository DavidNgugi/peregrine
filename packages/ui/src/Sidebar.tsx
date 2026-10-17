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
 * switches screens from state (PG-E7-01) and PG-E7-02 replaces that with typed links, at
 * which point the row becomes a link and keeps every other behaviour it has.
 *
 * **The column carries the window's top-left, and `header` is the caller's slot for it.** There is no
 * title bar of our own (§8 rules a fake one out), so the platform's window controls sit over this
 * column — the traffic lights on macOS — and the strip above the rows is where a person grabs the
 * window. The metrics and the drag region are the *caller's*: this package knows nothing about a
 * platform, so the slot is a node and the shell states the inset.
 *
 * **Two things collapse it, and they are not the same thing.** A window narrower than the `lg`
 * breakpoint falls back to the 60 px icon rail whatever the user asked for, because a 232 px column
 * in a 960 px window is a third of the screen; and `collapsed` is the user's own answer, which the
 * shell owns and a control toggles. The label stays in the accessibility tree at either width rather
 * than being hidden, because a row named by an icon alone cannot be read out.
 */
export interface SidebarProps {
  /** The landmark's accessible name; a page with two `nav` elements needs both named. */
  readonly label: string;
  /**
   * The strip above the rows: the caller's window chrome for whatever platform this is.
   *
   * It sits outside the scroller, so a control in it stays where it is while the screens scroll.
   */
  readonly header?: ReactNode;
  /**
   * The strip below the rows, pinned to the bottom.
   *
   * It sits outside the scroller for the same reason the header does: an identity control should be
   * reachable from every screen without scrolling the navigation to it.
   */
  readonly footer?: ReactNode;
  /** The user's own answer to "show me the labels", which the shell owns and a control toggles. */
  readonly collapsed?: boolean;
  /** The id a collapse control points at with `aria-controls`. */
  readonly id?: string;
  readonly children: ReactNode;
  readonly className?: string;
}

export function Sidebar({
  label,
  header,
  footer,
  collapsed = false,
  id,
  children,
  className,
}: SidebarProps): ReactElement {
  return (
    <nav
      aria-label={label}
      className={twMerge(
        clsx(
          "group/sidebar flex w-[232px] shrink-0 flex-col overflow-hidden border-r border-border-subtle bg-surface-1",
          collapsed ? "w-[60px]" : "max-lg:w-[60px]",
        ),
        className,
      )}
      data-collapsed={collapsed ? "true" : "false"}
      id={id}
    >
      {header}
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-2">{children}</div>
      {footer === undefined ? null : (
        <div className="shrink-0 border-t border-border-subtle p-2">{footer}</div>
      )}
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

/**
 * One screen in the sidebar.
 *
 * **The rail's two widths are one set of classes written twice, on purpose.** A collapsed column and
 * a narrow window put a row in the same state — icon centred, label kept for a screen reader — and
 * Tailwind has no "or" between a media query and an ancestor's attribute, so each rule names both.
 * The alternative is a bespoke stylesheet keyed on the attribute, which is a second way to style a
 * component in a kit that has one.
 */
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
          "max-lg:justify-center max-lg:px-0 group-data-[collapsed=true]/sidebar:justify-center group-data-[collapsed=true]/sidebar:px-0",
        ),
        className,
      )}
      onClick={onSelect}
      type="button"
    >
      <Icon name={icon} size={20} />
      {/* `sr-only` rather than `hidden` at rail width: it keeps the row's name. */}
      <span className="truncate max-lg:sr-only group-data-[collapsed=true]/sidebar:sr-only">
        {label}
      </span>
    </button>
  );
}
