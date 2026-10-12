import type { ReactElement, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { Platform } from "../lib/platform.js";

/**
 * The bar every platform's chrome is built from.
 *
 * `dragRegion` is handed to Tauri's injected script (`tauri/src/window/scripts/drag.js`),
 * which reads this attribute and walks the event's composed path: a click on any element
 * it classifies as clickable — `button`, `a`, `input`, `select`, `textarea`, `label`,
 * `summary`, `[role=button]`, `[tabindex]` — never starts a drag, however deep the region
 * is. That is what keeps the controls in the bar clickable, and it is also the hazard:
 * "deep" is what makes the bar's background and its title draggable, which is the part of
 * a title bar a user actually grabs, so anything interactive added to the chrome has to be
 * one of those elements (or be wrapped by one). "false" turns dragging off for the bar,
 * and for any ancestor that declares a region.
 *
 * The heights are each platform's own metric in logical pixels and not a spacing token
 * (docs/DESIGN_SYSTEM.md §4), because macOS and GNOME size a title bar and a header bar
 * themselves; logical pixels are what keep them the same size at 125%, 150%, and 200%
 * scaling. They are minimums, so a longer title or 1.4× text expansion grows the bar
 * instead of clipping it (AGENTS.md §4, Locale).
 */
function ChromeBar({
  showsName,
  dragRegion,
  className,
  children,
}: {
  readonly showsName: boolean;
  readonly dragRegion: "deep" | "false";
  readonly className: string;
  readonly children: ReactNode;
}): ReactElement {
  const { t } = useTranslation();
  const layout = showsName ? "justify-between" : "justify-end";

  return (
    <header
      className={`flex shrink-0 items-center gap-2 border-b border-border-subtle ${layout} ${className}`}
      data-tauri-drag-region={dragRegion}
    >
      {showsName ? <span className="text-title-2">{t("app.name")}</span> : null}
      {children}
    </header>
  );
}

/** macOS: the OS overlays the traffic lights on this bar and hides its own title, so this bar *is* the title bar. */
function MacOsChrome({ children }: { readonly children: ReactNode }): ReactElement {
  // 78 logical pixels is the traffic-light cluster plus the inset the OS gives it: the
  // title starts after them, and nothing interactive may sit under them.
  return (
    <ChromeBar className="min-h-[52px] pr-3 pl-[78px]" dragRegion="deep" showsName>
      {children}
    </ChromeBar>
  );
}

/**
 * Windows: the native frame keeps its caption buttons, its system menu, and its Snap
 * Layouts, none of which a web-drawn bar can reach, so this bar is the toolbar beneath
 * them and draws no window control. The OS title bar already shows the app name.
 */
function WindowsChrome({ children }: { readonly children: ReactNode }): ReactElement {
  return (
    <ChromeBar className="min-h-[40px] px-3" dragRegion="false" showsName={false}>
      {children}
    </ChromeBar>
  );
}

/** Linux: the desktop draws the window controls; the bar is a header bar sized like GNOME's. */
function LinuxChrome({ children }: { readonly children: ReactNode }): ReactElement {
  return (
    <ChromeBar className="min-h-[47px] px-3" dragRegion="false" showsName={false}>
      {children}
    </ChromeBar>
  );
}

/**
 * No platform was read, so no platform's geometry is assumed: a bar with no reserved
 * inset, no drag region, and the app name kept, because there is no OS title bar to show
 * it.
 */
function NeutralChrome({ children }: { readonly children: ReactNode }): ReactElement {
  return (
    <ChromeBar className="min-h-[40px] px-3" dragRegion="false" showsName>
      {children}
    </ChromeBar>
  );
}

export interface WindowChromeProps {
  /** The platform to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
  /** The app's own controls — navigation, theme — never window controls. */
  readonly children: ReactNode;
}

/**
 * The window chrome, per platform, behind one interface
 * (docs/ARCHITECTURE.md §17).
 *
 * A platform whose window management the OS still owns gets a toolbar and no window
 * control of our own: an invented caption button loses Snap Layouts on Windows and the
 * desktop's own controls on Linux (docs/research/desktop-ux.md §1.1), and
 * docs/DESIGN_SYSTEM.md §8 rules the fake title bar out directly. The window controls a
 * platform does draw are drawn over or beside this bar, which is why macOS reserves their
 * space and the others do not.
 */
export function WindowChrome({ platform, children }: WindowChromeProps): ReactElement {
  switch (platform) {
    case "macos":
      return <MacOsChrome>{children}</MacOsChrome>;
    case "windows":
      return <WindowsChrome>{children}</WindowsChrome>;
    case "linux":
      return <LinuxChrome>{children}</LinuxChrome>;
    default:
      return <NeutralChrome>{children}</NeutralChrome>;
  }
}
