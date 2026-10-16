/**
 * Writes the window's path, when there is a window.
 *
 * The application's route is state, and the path is how that state survives a reload: the entry reads
 * the path once at startup (`main.tsx`) and this keeps it in step afterwards, so the screen a user is
 * on is the screen the window says it is on. Without it the two drift the moment anyone navigates,
 * and a reload — which a webview does on its own for reasons it does not announce — would take the
 * user somewhere they were not.
 *
 * **It mirrors the current screen and builds no history of its own.** The places a user can go back
 * to are the shell's (`app/navigation.ts`), so a browser entry that grew with every sidebar click
 * would be a second, disagreeing stack — and the OS's own back gesture would walk it.
 *
 * The guard is not defensive noise: a Node test environment has no window, and the shell's tests
 * render the shell without one. Absence means there is no path to write, which is a fact rather than
 * a failure.
 */
export function writePath(path: string): void {
  if (typeof window === "undefined") {
    return;
  }
  window.history.replaceState(null, "", path);
}
