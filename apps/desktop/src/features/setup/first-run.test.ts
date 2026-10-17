import type { LibraryRootDto, ProviderDto, ProviderStatusDto } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";
import { isFirstRun } from "./first-run.js";

/** One provider as the core sends it. */
function provider(configured: boolean, name = "tmdb"): ProviderDto {
  return {
    provider: name,
    displayName: name.toUpperCase(),
    endpoint: `https://api.${name}.example/3`,
    configured,
  };
}

/** The provider answer, with the keychain's state and its own entries kept apart. */
function status(
  known: ProviderStatusDto["known"],
  store: ProviderStatusDto["store"] = "answered",
): ProviderStatusDto {
  return { known, store };
}

/** One library root. */
function root(id = 1): LibraryRootDto {
  return {
    id,
    path: `/media/library-${String(id)}`,
    kind: "mixed",
    scanMode: "incremental",
    lastScanAt: null,
    createdAt: 0,
  };
}

describe("isFirstRun", () => {
  it("is a first run when there is no folder and no configured provider", () => {
    expect(isFirstRun([], status([provider(false)]))).toBe(true);
  });

  it("is not a first run once a folder exists", () => {
    // A folder is enough to use the application: the scanner, the player, and the library screen
    // work without a provider, so a returner who skipped the key is not asked again.
    expect(isFirstRun([root()], status([provider(false)]))).toBe(false);
  });

  it("is not a first run once a provider is configured", () => {
    // A provider is enough to browse the catalogue even before a folder has been chosen.
    expect(isFirstRun([], status([provider(false), provider(true, "other")]))).toBe(false);
  });

  it("is not a first run when both exist", () => {
    expect(isFirstRun([root()], status([provider(true)]))).toBe(false);
  });

  it("treats a build with no known provider as nothing to configure", () => {
    // The empty list is not "nobody has configured it"; it is "this build has none". Asking the
    // user to configure one would be asking for something the application cannot accept.
    expect(isFirstRun([], status([]))).toBe(true);
  });

  it("does not turn a library owner back into a first run when the keychain refused", () => {
    // The exact defect the settings screen exists to prevent: macOS binds a keychain item's
    // access to the code signature, so a rebuilt binary can be refused and every `configured`
    // flag reads false. The root still proves this installation was set up.
    expect(isFirstRun([root()], status([provider(false)], "denied"))).toBe(false);
  });

  it("is still a first run with no folder when the keychain could not be read", () => {
    // With no root, the wizard is the only route to a usable application; the provider step's own
    // store notice explains why a key cannot be read or stored.
    expect(isFirstRun([], status([provider(false)], "denied"))).toBe(true);
    expect(isFirstRun([], status([provider(false)], "unread"))).toBe(true);
  });
});
