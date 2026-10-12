import type { ShortcutBinding } from "@peregrine/ipc-client";

/**
 * The part of a keyboard event a shortcut is matched against.
 *
 * `KeyboardEvent` satisfies this structurally, and so does a plain object in a test, so
 * the matcher needs neither a DOM nor a synthetic event to be exercised.
 */
export interface KeyPress {
  readonly code: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
}

/**
 * The registered shortcut a key press is, or `null`.
 *
 * Every modifier is compared, in both directions: `Cmd+Shift+1` must not fire
 * `navigation.discover`, and a binding that wants Shift must not fire without it. The
 * modifier names arrive already resolved for the running platform — the core built them
 * from `muda`'s `CMD_OR_CTRL`, so `meta` is the Command key on macOS and Control is
 * `ctrl` there — which is what keeps this comparison free of a platform table.
 */
export function shortcutFor(
  bindings: readonly ShortcutBinding[],
  press: KeyPress,
): ShortcutBinding | null {
  return (
    bindings.find(
      (binding) =>
        binding.code === press.code &&
        binding.meta === press.metaKey &&
        binding.ctrl === press.ctrlKey &&
        binding.alt === press.altKey &&
        binding.shift === press.shiftKey,
    ) ?? null
  );
}
