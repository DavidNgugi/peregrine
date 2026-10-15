import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App.js";
import { HOME, parseRoute } from "./app/routes.js";
import { resolvePlatform } from "./lib/platform.js";
import "./styles.css";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html has no #root element; the app cannot mount.");
}

// The platform decides the chrome's geometry, so it is read before the first render:
// painting a neutral bar and then reflowing into the right one is a visible jump at
// startup, and the read is an in-process IPC lookup rather than a network call. It cannot
// reject; an unreadable platform becomes `null` and the shell keeps the neutral bar
// (docs/ARCHITECTURE.md §15).
void resolvePlatform().then((platform) => {
  createRoot(container).render(
    <StrictMode>
      {/* The path the window opened at, read here rather than inside the shell: `window` belongs
          to the browser entry, and the shell stays renderable without one (its tests have none).
          A path this build has no screen for opens the home screen, which is what a browser does
          with a URL it cannot resolve. */}
      <App
        platform={platform}
        route={parseRoute(window.location.pathname + window.location.search) ?? HOME}
      />
    </StrictMode>,
  );
});
