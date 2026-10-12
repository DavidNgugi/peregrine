import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Gallery } from "./Gallery.js";
import "../styles.css";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("gallery.html has no #root element; the gallery cannot mount.");
}

createRoot(container).render(
  <StrictMode>
    <Gallery />
  </StrictMode>,
);
