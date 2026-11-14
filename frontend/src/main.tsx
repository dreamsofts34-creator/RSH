import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import "./lib/i18n";
import { applyTheme, loadStoredTheme } from "./lib/theme";
import App from "./App";

// Apply the stored theme BEFORE first paint so the window never flashes
// the wrong theme (and never paints unstyled).
applyTheme(loadStoredTheme());

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("RSH: #root element is missing from index.html");
}

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
