import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));

// RSH frontend — Vite + React + Tailwind v4. Tauri dev server port is 1420
// (strictPort: `tauri dev` expects the dev server on this port).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(root, "src"),
    },
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    // Tauri serves dist/ from disk; relative asset paths keep it working
    // regardless of the window URL.
    outDir: "dist",
    emptyOutDir: true,
  },
});
