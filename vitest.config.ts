import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// Internal packages are consumed from source so that a clean checkout can run
// `pnpm test` and `pnpm typecheck` without building `dist/` first; a packaged
// build consumes the same packages through their `exports` map. Unlike the app's
// Vite config these patterns need no anchoring: no test loads the package CSS.
const packageAliases = {
  "@peregrine/i18n": "./packages/i18n/src/index.ts",
  "@peregrine/ipc-client": "./packages/ipc-client/src/index.ts",
  "@peregrine/player-ui": "./packages/player-ui/src/index.ts",
  "@peregrine/theme": "./packages/theme/src/index.ts",
  "@peregrine/ui": "./packages/ui/src/index.ts",
} as const;

export default defineConfig({
  resolve: {
    alias: Object.fromEntries(
      Object.entries(packageAliases).map(([name, path]) => [
        name,
        fileURLToPath(new URL(path, import.meta.url)),
      ]),
    ),
  },
  test: {
    environment: "node",
    include: ["packages/*/src/**/*.test.{ts,tsx}", "apps/*/src/**/*.test.{ts,tsx}"],
  },
});
