import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

/**
 * Whether this window is on the platform's own full screen.
 *
 * **The shell asks because the answer decides where the sidebar's first control can sit.** A macOS
 * window in the `Overlay` title-bar style draws the traffic lights *over* the content, so the
 * sidebar's top has to leave their band empty; a macOS *full screen* hides them until the pointer
 * reaches the top edge, and leaving the band empty there is 20 px of nothing between the collapse
 * toggle and the search field beside it — which is exactly what a person notices.
 *
 * Outside the Tauri shell there is no window to ask: a browser preview and every test render the
 * shell with no bridge, so `false` is the answer rather than a failure, and nothing here reports an
 * error for a state that does not exist.
 */
export function useFullscreen(): boolean {
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    const window = getCurrentWindow();
    let cancelled = false;
    const read = (): void => {
      void window.isFullscreen().then((value) => {
        if (!cancelled) {
          setFullscreen(value);
        }
      });
    };
    read();
    // A transition to or from a full screen is a resize, so one subscription covers both directions
    // and the macOS menu's own "Enter Full Screen" as well as the traffic light's.
    const unlisten = window.onResized(read);
    return () => {
      cancelled = true;
      void unlisten.then((off) => {
        off();
      });
    };
  }, []);

  return fullscreen;
}
