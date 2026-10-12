/**
 * The typed frontend API for the OS notification permission (PG-E1-20).
 *
 * This module is the only place in the app that names
 * `@tauri-apps/plugin-notification`, and it names exactly two of its exports:
 * `isPermissionGranted` and `requestPermission`. `sendNotification` is deliberately absent
 * — a toast is shown by the core through `OsNotifier`, which writes the in-app centre entry
 * first, and a renderer that could send one directly would be a path around that invariant
 * (AGENTS R31). The capability grants the two commands these calls use and nothing else.
 *
 * **A granted probe is not proof the user will see anything.** ADR 0015 §2 is explicit
 * about this: on desktop the plugin's own `request_permission` returns `Granted`
 * unconditionally, the JavaScript `isPermissionGranted()` consults `window.Notification`
 * first, macOS may have the app switched off in System Settings, Windows only delivers for
 * an installed app, and Linux may have no notification daemon at all. So a `granted` result
 * means "nothing here says no", which is why the settings screen carries a test button and
 * a link to the OS page rather than a claim.
 *
 * The two calls are separate on purpose. `probePermission` never prompts, so Settings can
 * read the state on every mount; `requestPermission` is the ask, and only the consent step
 * — gated by the core's `shouldPrompt` — may call it.
 */

import { normalizeIpcError } from "@peregrine/ipc-client";
import { isTauri } from "@tauri-apps/api/core";
import {
  isPermissionGranted,
  requestPermission as requestSystemPermission,
} from "@tauri-apps/plugin-notification";
import { logUiError } from "./error-log.js";
import type { Platform } from "./platform.js";

/** What the OS says about showing this app's notifications. */
export type PermissionProbe =
  /** The system reports that notifications are allowed. */
  | { readonly kind: "granted" }
  /** The system reports that they are refused. */
  | { readonly kind: "denied" }
  /**
   * There is no answer: outside the Tauri shell, a call that failed, or an ask the user
   * dismissed without choosing. Never treated as granted.
   */
  | { readonly kind: "unverified" };

/**
 * Reads the permission the OS reports, without asking the user for anything.
 *
 * Safe to call whenever the state is needed: the plugin's implementation only reads
 * `window.Notification.permission` and never raises a prompt.
 */
export async function probePermission(): Promise<PermissionProbe> {
  if (!isTauri()) {
    return { kind: "unverified" };
  }
  try {
    return (await isPermissionGranted()) ? { kind: "granted" } : { kind: "denied" };
  } catch (caught) {
    logUiError("probePermission", normalizeIpcError(caught));
    return { kind: "unverified" };
  }
}

/**
 * Asks the OS for permission, which on macOS is what raises the system prompt.
 *
 * The caller must be a path the core's `should_prompt_consent` allowed: a user who declined
 * is never asked again unprompted, and calling this without that answer would be the nag ADR
 * 0015 §2 forbids. A `"default"` answer means the user dismissed the prompt without
 * choosing, which is reported as `unverified` and never as `granted`.
 */
export async function requestPermission(): Promise<PermissionProbe> {
  if (!isTauri()) {
    return { kind: "unverified" };
  }
  try {
    const answer = await requestSystemPermission();
    switch (answer) {
      case "granted":
        return { kind: "granted" };
      case "denied":
        return { kind: "denied" };
      default:
        return { kind: "unverified" };
    }
  } catch (caught) {
    logUiError("requestPermission", normalizeIpcError(caught));
    return { kind: "unverified" };
  }
}

/**
 * The OS page where the user can change this app's notification settings, when there is one.
 *
 * macOS and Windows each have a documented URL scheme for their settings app. Linux has no
 * standard one — GNOME's and KDE's notification panes are reached differently and neither
 * publishes a portable URL — so this answers `null` there and the settings screen has to say
 * so in words rather than offer a link that goes nowhere. `null` also covers a platform the
 * core could not name.
 *
 * Both URLs are unverified in this build: opening one would change the user's system
 * settings pane, which no test may do (SPIKE-11 records what each OS actually accepts).
 */
export function notificationSettingsUrl(platform: Platform | null): string | null {
  switch (platform) {
    case "macos":
      return "x-apple.systempreferences:com.apple.Notifications-Settings.extension";
    case "windows":
      return "ms-settings:notifications";
    default:
      return null;
  }
}
