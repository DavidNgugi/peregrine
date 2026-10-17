import type { ReactElement } from "react";
import { useState } from "react";
import { SetupWizard, useFirstRun, type FirstRunCheck } from "../features/setup/index.js";
import type { Platform } from "../lib/platform.js";
import { AppShell } from "./AppShell.js";
import type { Route } from "./routes.js";

interface SetupGateProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
  /** The route the window opened at, which the shell owns from then on. */
  readonly route: Route;
}

interface SetupGateViewProps extends SetupGateProps {
  readonly check: FirstRunCheck;
}

/**
 * Which of the two first frames the window draws: the wizard, or the application.
 *
 * **Only `first-run` shows the wizard.** `checking` and `unknown` render the shell: a read that
 * has not answered is not evidence of a first run, and a read that failed is even less so. The
 * first-run read is two local calls, and a returner who saw a setup screen because a database
 * call was slow would be told to configure what they already had. The cost of the other
 * direction is a first-run user seeing the shell for one frame, which is invisible and reversible;
 * this one is neither.
 *
 * The decision lives here, in the router/shell layer, rather than in a screen: the wizard
 * replaces the sidebar, the player bar, and the screen switch together, and a route that drew it
 * inside `main` would leave the navigation pointing at screens that have nothing to show.
 */
export function SetupGate({ platform, route }: SetupGateProps): ReactElement {
  const check = useFirstRun();
  return <SetupGateView check={check} platform={platform} route={route} />;
}

/**
 * The gate's decision, separated from the read so both first frames can be rendered in a test.
 *
 * `finished` is session state rather than a second IPC read: the wizard is finished when the
 * user says so on its last step, and re-reading the setup facts at that moment is exactly the
 * read that would have swapped the wizard away mid-step.
 */
export function SetupGateView({ check, platform, route }: SetupGateViewProps): ReactElement {
  const [finished, setFinished] = useState(false);

  if (check.state === "first-run" && !finished) {
    return (
      <SetupWizard
        onFinished={() => {
          setFinished(true);
        }}
      />
    );
  }

  return <AppShell platform={platform} route={route} />;
}
