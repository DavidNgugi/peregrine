import { createI18n } from "@peregrine/i18n";
import type { ReactElement } from "react";
import { I18nextProvider } from "react-i18next";
import type { Platform } from "../lib/platform.js";
import { AppErrorBoundary } from "./AppErrorBoundary.js";
import { SetupGate } from "./SetupGate.js";
import type { Route } from "./routes.js";

// The catalogue is bundled, so the instance is built once per process and the
// first paint already has copy.
const i18n = createI18n();

interface AppProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
  /** The route the window opened at, which the shell owns from then on. */
  readonly route: Route;
}

export function App({ platform, route }: AppProps): ReactElement {
  return (
    <I18nextProvider i18n={i18n}>
      <AppErrorBoundary>
        <SetupGate platform={platform} route={route} />
      </AppErrorBoundary>
    </I18nextProvider>
  );
}
