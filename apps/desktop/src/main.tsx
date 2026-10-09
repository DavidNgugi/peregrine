import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App.js";
import "./styles.css";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html has no #root element; the app cannot mount.");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
