import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import "./lib/i18n";
import { applyStoredLanguage } from "./lib/i18n";
import {
  applyTheme,
  installSystemThemeWatcher,
  loadStoredTheme,
} from "./lib/theme";
import App from "./App";

// Apply the stored theme BEFORE first paint so the window never flashes
// the wrong theme (and never paints unstyled).
applyTheme(loadStoredTheme());
// Follow the OS scheme while the theme is "system" (M14).
installSystemThemeWatcher();
// Apply the stored language (and document direction) before first
// paint, same as the theme (H2).
applyStoredLanguage();

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("RSH: #root element is missing from index.html");
}

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
