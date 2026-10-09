import { createI18n } from "@peregrine/i18n";
import type { ReactElement } from "react";
import { I18nextProvider } from "react-i18next";
import { AppErrorBoundary } from "./AppErrorBoundary.js";
import { AppShell } from "./AppShell.js";

// The catalogue is bundled, so the instance is built once per process and the
// first paint already has copy.
const i18n = createI18n();

export function App(): ReactElement {
  return (
    <I18nextProvider i18n={i18n}>
      <AppErrorBoundary>
        <AppShell />
      </AppErrorBoundary>
    </I18nextProvider>
  );
}
