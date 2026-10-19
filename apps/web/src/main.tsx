import "@peregrine/theme/tokens.css";
import "./styles.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App.js";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("the document has no root element");
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
