/**
 * Which shape the library screen draws: a table of counts, or a wall of posters.
 *
 * **Both drawings read the same answer, which is why this is a view rather than a second read.** The
 * table is what a person consults to compare sizes and spot copies a scan could not find; the grid is
 * what a person consults to recognise a title by its cover. Neither is a filter — the same titles, in
 * the same order, are behind both — so the choice belongs to the screen and never reaches the core
 * (`AGENTS R7`: there is no rule here to compute).
 *
 * **It is shell state rather than component state, and that is the one non-obvious decision.** `main`
 * in `AppShell` renders exactly one screen, so opening a title and coming back unmounts the library
 * and a `useState` would reset the drawing to its default on every visit — a view control that does
 * not stick is a control a reader stops using. It lives in a store for the reason `theme-store` does,
 * and it is deliberately *not* persisted: lasting across restarts is a setting, and settings are
 * `PG-E20-01`'s registry. "Sticky for the session" is the honest scope of what this owns.
 *
 * **The grid is the default**, because the question a media library is opened to answer is "what have
 * I got" and a cover answers it faster than a byte count. The table keeps its own defaults and is one
 * press away.
 */

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/** The two drawings the library offers. */
export type LibraryView = "list" | "grid";

interface LibraryViewState {
  readonly view: LibraryView;
  readonly setView: (view: LibraryView) => void;
}

export const libraryViewStore = createStore<LibraryViewState>((set) => ({
  view: "grid",
  setView: (view) => {
    set({ view });
  },
}));

/** What the toggle and the body of the screen read. */
export interface LibraryViewControls {
  readonly view: LibraryView;
  readonly setView: (view: LibraryView) => void;
}

/** Which drawing is on screen, and how to change it. */
export function useLibraryView(): LibraryViewControls {
  const view = useStore(libraryViewStore, (state) => state.view);
  const setView = useStore(libraryViewStore, (state) => state.setView);
  return { view, setView };
}
