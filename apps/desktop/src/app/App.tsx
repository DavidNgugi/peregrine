import { createI18n } from "@peregrine/i18n";
import { useEffect, type ReactElement } from "react";
import { I18nextProvider } from "react-i18next";
import type { Platform } from "../lib/platform.js";
import { WEB_AUTO_KEY, startWebAccess } from "../lib/webAccess.js";
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
  // The switch is an app-level promise, not a Settings-section one: a launch with it on has to
  // start the listener before anybody opens Settings. One retry covers the core still coming up.
  useEffect(() => {
    if (typeof localStorage === "undefined" || localStorage.getItem(WEB_AUTO_KEY) !== "1") {
      return;
    }
    void startWebAccess().catch(() => {
      window.setTimeout(() => {
        void startWebAccess().catch(() => {});
      }, 2000);
    });
  }, []);

  return (
    <I18nextProvider i18n={i18n}>
      <AppErrorBoundary>
        <SetupGate platform={platform} route={route} />
      </AppErrorBoundary>
    </I18nextProvider>
  );
}
