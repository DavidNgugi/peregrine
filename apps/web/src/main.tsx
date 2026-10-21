import "@peregrine/theme/tokens.css";
import "./styles.css";
import "./home.css";
import "./title.css";
import "./player.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App.js";
import { isTizen } from "./tizen.js";

// The class is set before the first paint so the ten-foot focus ring and the hidden pointer are
// there on the initial screen, not one render later. A phone or desktop never gets it.
if (isTizen(navigator.userAgent)) {
  document.documentElement.classList.add("tizen");
}

const root = document.getElementById("root");
if (root === null) {
  throw new Error("the document has no root element");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
