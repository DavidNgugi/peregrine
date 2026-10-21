/**
 * The Tizen remote's keys that a DOM key event does not cover (ADR 0034).
 *
 * The Tizen shell navigates top-level to this client, so this page is Peregrine's own origin and
 * cannot be injected from the shell: the Back key has to be handled here. The transport keys are
 * handled by the player itself, because a listener that exists only while the player is mounted is
 * the "only when a player is open" rule with no second condition to keep in step.
 */

import { useEffect, useRef } from "react";

/** A transport action a television remote can send without a focused control. */
export type TransportAction = "play" | "pause" | "toggle" | "stop" | "forward" | "rewind";

/** The screens the client can be on, as the Back decision names them. */
export type ClientScreen =
  "loading" | "pair" | "home" | "browse" | "myList" | "downloads" | "library" | "title" | "player";

/** Where a hardware Back press should land. */
export type BackTarget = "player" | "back" | "exit";

/** Whether this user agent is a Tizen television rather than a phone or desktop browser. */
export function isTizen(userAgent: string): boolean {
  return /Tizen/i.test(userAgent);
}

/** Whether a `tizenhwkey` event is the Back key. */
export function isHardwareBack(keyName: string | undefined): boolean {
  return keyName === "back";
}

/** The key code Tizen sends for Back when it arrives as a key event instead. */
export const BACK_KEY_CODE = 10_009;

/** Whether a keydown is Back, for the engines that send it there instead of `tizenhwkey`. */
export function isBackKey(keyCode: number): boolean {
  return keyCode === BACK_KEY_CODE;
}

/**
 * The transport keys, by the Tizen code and by the DOM key name.
 *
 * Values are the Tizen TV key codes (415/19/10252/413/417/412); the names are what a Chromium
 * build reports for the same remote buttons when it reports a name at all.
 */
export function transportAction(keyCode: number, key?: string): TransportAction | null {
  switch (key) {
    case "MediaPlayPause":
      return "toggle";
    case "MediaPlay":
      return "play";
    case "MediaPause":
      return "pause";
    case "MediaStop":
      return "stop";
    case "MediaFastForward":
      return "forward";
    case "MediaRewind":
      return "rewind";
  }
  switch (keyCode) {
    case 10_252:
      return "toggle";
    case 415:
      return "play";
    case 19:
      return "pause";
    case 413:
      return "stop";
    case 417:
      return "forward";
    case 412:
      return "rewind";
    default:
      return null;
  }
}

/**
 * What Back means on each screen.
 *
 * At the root — home, the pairing screen, or the first paint — there is nowhere left to go, so
 * Back leaves the app. The player closes back to its title; every other screen pops the app's own
 * history. Treating home as "history" would walk back into a screen the person already left.
 */
export function backTarget(screen: ClientScreen): BackTarget {
  if (screen === "player") return "player";
  if (screen === "home" || screen === "pair" || screen === "loading") return "exit";
  return "back";
}

/**
 * Leaves the Tizen application, and says whether it could.
 *
 * `false` is not a failure to report: the page may be running in a browser that has no Tizen
 * application object, and the caller then leaves the Back key to the platform rather than
 * swallowing a press that would otherwise close the app.
 */
export function exitTizenApp(): boolean {
  const root = window as Window & {
    readonly tizen?: {
      readonly application?: {
        getCurrentApplication?: () => { exit?: () => void } | undefined;
      };
    };
  };
  try {
    const application = root.tizen?.application?.getCurrentApplication?.();
    if (application?.exit === undefined) return false;
    application.exit();
    return true;
  } catch {
    return false;
  }
}

/**
 * Handles the hardware Back key while the client is mounted.
 *
 * Some Tizen builds fire `tizenhwkey` and a keydown for the same press; the short debounce turns
 * that into one navigation and repeats its answer, so a duplicate is cancelled or passed through
 * exactly like the press it copies. `preventDefault` is what asks the platform not to close the app
 * by itself, which would make Back on a title page quit instead of returning to the screen behind
 * it. The callback returns whether it handled the press; at the root with no Tizen application
 * object it returns false and the platform's own Back closes the app.
 */
export function useTizenBack(enabled: boolean, onBack: () => boolean): void {
  const handler = useRef(onBack);
  handler.current = onBack;

  useEffect(() => {
    if (!enabled) return;
    let lastHandled = 0;
    let lastResult = false;
    const run = (): boolean => {
      const now = Date.now();
      if (now - lastHandled < 300) return lastResult;
      lastHandled = now;
      lastResult = handler.current();
      return lastResult;
    };
    const onHardware = (event: Event): void => {
      const keyName = (event as Event & { readonly keyName?: string }).keyName;
      if (!isHardwareBack(keyName)) return;
      if (run()) event.preventDefault();
    };
    const onKey = (event: KeyboardEvent): void => {
      if (!isBackKey(event.keyCode)) return;
      if (run()) event.preventDefault();
    };
    window.addEventListener("tizenhwkey", onHardware);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("tizenhwkey", onHardware);
      window.removeEventListener("keydown", onKey);
    };
  }, [enabled]);
}

/** The box a focus plate occupies around a target, in viewport coordinates. */
export function focusPlateBox(
  rect: { readonly x: number; readonly y: number; readonly width: number; readonly height: number },
  inset = 4,
): { readonly x: number; readonly y: number; readonly width: number; readonly height: number } {
  return {
    x: rect.x - inset,
    y: rect.y - inset,
    width: rect.width + inset * 2,
    height: rect.height + inset * 2,
  };
}

/**
 * One floating focus ring for the television, moved with `transform` and `opacity`.
 *
 * The reference set measured a per-tile focus class at roughly 5 fps and a single moved plate at
 * 60 fps; the plate is why this is one element outside React rather than a class on every tile.
 * The size and radius are written only when the target changes; the hot path — a scroll or a
 * D-pad move — writes only the transform. Focus is the television's only pointer, so there is no
 * hover highlight to keep in step with it.
 */
export function useTizenFocusPlate(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return;
    const plate = document.createElement("div");
    plate.id = "tizen-focus";
    plate.setAttribute("aria-hidden", "true");
    document.body.append(plate);
    document.documentElement.dataset["tvFocus"] = "true";

    let frame = 0;
    let lastTransform = "";
    let lastTarget: Element | null = null;
    let lastRadius = "";
    const setVisible = (visible: boolean): void => {
      const value = visible ? "true" : "false";
      if (plate.dataset["visible"] !== value) plate.dataset["visible"] = value;
    };
    const update = (): void => {
      frame = 0;
      const target = document.activeElement;
      if (!(target instanceof HTMLElement) || !target.matches("[data-focus]")) {
        setVisible(false);
        return;
      }
      const rect = target.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) {
        setVisible(false);
        return;
      }
      const box = focusPlateBox(rect);
      const width = box.width + "px";
      const height = box.height + "px";
      if (plate.style.width !== width) plate.style.width = width;
      if (plate.style.height !== height) plate.style.height = height;
      if (target !== lastTarget) {
        lastTarget = target;
        // A card's own border-radius is 0; the visible poster is the rounded art. Follow the
        // shape a person sees, not the transparent button around it. The read is per target,
        // not per scroll frame.
        const art = target.querySelector<HTMLElement>(".card-art");
        lastRadius = getComputedStyle(art ?? target).borderRadius;
        if (plate.style.borderRadius !== lastRadius) plate.style.borderRadius = lastRadius;
      }
      const transform = "translate3d(" + box.x + "px, " + box.y + "px, 0)";
      if (lastTransform !== transform) {
        lastTransform = transform;
        plate.style.transform = transform;
      }
      setVisible(true);
    };
    const schedule = (): void => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };
    const onFocus = (): void => schedule();
    const onScroll = (): void => schedule();
    // Capture runs before the spatial handler moves focus; the frame it schedules runs after,
    // so a focus change made by an arrow key is never missed by a missing focusin event.
    const onKey = (): void => schedule();

    document.addEventListener("focusin", onFocus);
    document.addEventListener("focusout", onFocus);
    window.addEventListener("focus", onFocus, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", onScroll, { capture: true, passive: true });
    window.addEventListener("resize", onScroll);
    // The hero rotates in place and a layout can move the focused control without a scroll or
    // resize. A coarse geometry check follows it; the write guards above keep it from touching
    // the style system unless the transform actually changed.
    const poll = window.setInterval(schedule, 150);
    schedule();

    return () => {
      document.removeEventListener("focusin", onFocus);
      document.removeEventListener("focusout", onFocus);
      window.removeEventListener("focus", onFocus, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", onScroll, { capture: true });
      window.removeEventListener("resize", onScroll);
      window.clearInterval(poll);
      if (frame !== 0) cancelAnimationFrame(frame);
      plate.remove();
      delete document.documentElement.dataset["tvFocus"];
    };
  }, [enabled]);
}
