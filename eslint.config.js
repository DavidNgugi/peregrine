// ESLint flat config. Base JS rules come from @eslint/js; TypeScript rules from
// typescript-eslint. Type-aware linting is deliberately absent: the parser would
// then need the TypeScript 6 compiler API (see docs/DEPENDENCIES.md §7), and the
// syntax-level rules below are what PG-E0-09 asks for.
import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      "**/dist/**",
      "**/node_modules/**",
      "**/coverage/**",
      // The Rust workspace's target directory is at the repository root, and
      // Tauri's build script drops a generated `__global-api-script.js` inside
      // it. ESLint flat config does not read .gitignore, so without this line a
      // build before a lint run fails the lint gate on a generated file.
      "target/**",
      "**/target/**",
      // Generated from the Rust IPC types by `cargo xtask gen-ipc`; the
      // generator's output uses `any` in its typed-error runtime and comes from
      // the Rust compiler's view of the command surface, not from this config.
      "packages/ipc-client/src/generated/**",
      "apps/desktop/src-tauri/gen/**",
      "apps/desktop/src-tauri/target/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: { ...globals.browser },
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/consistent-type-imports": "error",
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
      // No react plugin is installed, so the two XSS sinks are banned by AST
      // selector rather than by an extra dependency.
      "no-restricted-syntax": [
        "error",
        {
          selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
          message: "Markup is never injected as HTML (SECURITY.md T8).",
        },
        {
          selector: "MemberExpression[property.name='dangerouslySetInnerHTML']",
          message: "Markup is never injected as HTML (SECURITY.md T8).",
        },
      ],
    },
  },
  {
    files: ["**/*.{js,mjs,cjs}", "**/*.test.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
  {
    // The Tizen shell is a packaged classic script running in a browser webview, not a Node
    // module. ES5 is deliberate: the engine is Chromium 76 and there is no transpiler.
    files: ["apps/tizen/**/*.js"],
    languageOptions: {
      ecmaVersion: 5,
      sourceType: "script",
      globals: { ...globals.browser },
    },
    rules: {
      // ES5 has no optional catch binding, and these catches are deliberate no-ops.
      "@typescript-eslint/no-unused-vars": ["error", { caughtErrors: "none" }],
    },
  },
);
