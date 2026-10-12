/**
 * The typed frontend API for the OS file and folder pickers (PG-E1-12).
 *
 * This module is the only place in the app that names `@tauri-apps/plugin-dialog`. A
 * feature that needs a path calls `pickFile` or `pickFolder` here, so the plugin can be
 * swapped, the return type can stay honest, and — the reason that matters — there is one
 * place to audit for what a picker result is allowed to become.
 *
 * **A picked path is not trusted input.** The dialog proves only that a human chose the
 * path in an OS-owned window; it proves nothing about the path itself, and the value
 * crosses the webview boundary as a string like any other. Validation is not this module's
 * job and cannot be: `PathGuard` is `peregrine-domain`'s, and containment, symlinks, and
 * canonicalization are decided in Rust (SECURITY.md, "Filesystem"). So this module hands
 * back `UntrustedPath`, a branded string that cannot be passed where a validated path is
 * wanted, and the IPC command that receives it re-validates it against the caller's
 * configured roots before it is used. A picker result is a *candidate*, and the first
 * thing that reads it must be the guard.
 */

import { isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

/**
 * A path the user chose in an OS dialog, before any policy has been applied to it.
 *
 * The brand is what stops a picked path from being handed to something that expected a
 * path `PathGuard` produced: `SafePath` in Rust has no TypeScript twin, so the two sides
 * meet at an IPC command whose argument is this type and whose body calls the guard.
 */
export type UntrustedPath = string & { readonly __untrustedPath: unique symbol };

/** Whether this path came from a picker rather than from stored, already-guarded data. */
export function isUntrustedPath(value: unknown): value is UntrustedPath {
  return typeof value === "string" && value.length > 0;
}

/** One entry in the dialog's filter selector. */
export interface PickerFilter {
  /** The label the OS shows in the filter dropdown. */
  readonly name: string;
  /** Extensions without the leading dot, exactly as the dialog API wants them. */
  readonly extensions: readonly string[];
}

/** What every picker accepts. */
interface PickOptions {
  /** The dialog's own title, where the platform shows one. */
  readonly title?: string;
  /** Where the dialog opens, as a directory the caller has already validated. */
  readonly startIn?: string;
  /** Filters for the platform's dropdown; ignored by some platform pickers. */
  readonly filters?: readonly PickerFilter[];
}

/** Options for [`pickFile`]. */
export interface FilePickerOptions extends PickOptions {
  /** Allow more than one file; the result is then an array. */
  readonly multiple?: boolean;
}

/** What a file picker can answer: nothing, one path, or several. */
export type FilePickerResult =
  | { readonly kind: "cancelled" }
  | { readonly kind: "files"; readonly paths: readonly UntrustedPath[] };

/** What a folder picker can answer. */
export type FolderPickerResult =
  { readonly kind: "cancelled" } | { readonly kind: "folder"; readonly path: UntrustedPath };

/**
 * Asks the OS for one or more files.
 *
 * `null` means the user cancelled, which is a normal outcome and not an error: a caller
 * gets a discriminated result rather than a thrown one, so "the user changed their mind"
 * cannot be handled by the same branch as "the dialog failed to open".
 */
export async function pickFile(options: FilePickerOptions = {}): Promise<FilePickerResult | null> {
  if (!isTauri()) {
    // A browser preview of the frontend has no dialog to open, and inventing a path would
    // be worse than reporting that there is none.
    return null;
  }
  const selection = await open({
    directory: false,
    multiple: options.multiple ?? false,
    ...(options.title === undefined ? {} : { title: options.title }),
    ...(options.startIn === undefined ? {} : { defaultPath: options.startIn }),
    ...(options.filters === undefined ? {} : { filters: toFilters(options.filters) }),
  });

  if (selection === null) {
    return { kind: "cancelled" };
  }
  // The dialog reports a single selection as a string and a multiple selection as an
  // array, and only for `multiple: true` is the string impossible; normalising here is
  // what keeps every caller from repeating the same union narrowing.
  const paths = (Array.isArray(selection) ? selection : [selection]).filter(isUntrustedPath);
  return paths.length === 0 ? { kind: "cancelled" } : { kind: "files", paths };
}

/**
 * Asks the OS for one folder.
 *
 * Windows shows a separate dialog for folders, so the caller is the one that decides
 * whether a folder is what it wants; asking for "a file or a folder" is not an OS concept
 * there (`mergesFileAndFolderPickers` in `platform.ts`).
 */
export async function pickFolder(options: PickOptions = {}): Promise<FolderPickerResult | null> {
  if (!isTauri()) {
    return null;
  }
  const selection = await open({
    directory: true,
    multiple: false,
    recursive: false,
    ...(options.title === undefined ? {} : { title: options.title }),
    ...(options.startIn === undefined ? {} : { defaultPath: options.startIn }),
  });

  if (selection === null || Array.isArray(selection) || !isUntrustedPath(selection)) {
    return { kind: "cancelled" };
  }
  return { kind: "folder", path: selection };
}

function toFilters(filters: readonly PickerFilter[]): { name: string; extensions: string[] }[] {
  return filters.map((filter) => ({
    name: filter.name,
    extensions: [...filter.extensions],
  }));
}
