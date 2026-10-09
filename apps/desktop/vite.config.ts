import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

// Internal packages are consumed from source so a clean checkout builds and
// typechecks before anything writes `dist/`; the same paths are in
// tsconfig.base.json so the editor resolves them identically.
//
// The patterns are anchored because subpath imports (`@peregrine/theme/tokens.css`)
// have to fall through to the package's own `exports` map.
const packageAliases: readonly (readonly [RegExp, string])[] = [
  [/^@peregrine\/i18n$/u, "../../packages/i18n/src/index.ts"],
  [/^@peregrine\/ipc-client$/u, "../../packages/ipc-client/src/index.ts"],
  [/^@peregrine\/player-ui$/u, "../../packages/player-ui/src/index.ts"],
  [/^@peregrine\/theme$/u, "../../packages/theme/src/index.ts"],
  [/^@peregrine\/ui$/u, "../../packages/ui/src/index.ts"],
];

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: packageAliases.map(([find, path]) => ({
      find,
      replacement: fileURLToPath(new URL(path, import.meta.url)),
    })),
  },
  // Tauri owns the terminal when it runs `pnpm dev`; clearing it would hide the
  // native build output that comes after the frontend is ready.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
});
