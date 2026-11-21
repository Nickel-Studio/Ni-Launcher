import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

export default defineConfig({
  root: fileURLToPath(new URL("../../", import.meta.url)),
  cacheDir: fileURLToPath(new URL("./.vite", import.meta.url)),
  plugins: [react()],
  clearScreen: false,
  optimizeDeps: {
    entries: ["tests/export-regression/regression.html"],
    include: [
      "react",
      "react/jsx-runtime",
      "react/jsx-dev-runtime",
      "react-dom/client",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/plugin-opener",
    ],
    noDiscovery: true,
  },
  server: {
    host: "127.0.0.1",
    hmr: false,
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/tests/export-regression/backend-evidence/**",
      ],
    },
  },
});
