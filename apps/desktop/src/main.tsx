import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App.js";
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
      <App platform={platform} />
    </StrictMode>,
  );
});
