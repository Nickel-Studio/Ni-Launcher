import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  cacheDir: "node_modules/.vite-community-dependencies",
  optimizeDeps: {
    entries: ["tests/community-dependencies/regression.html"],
    include: [
      "react",
      "react-dom/client",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/plugin-opener",
    ],
  },
  server: {
    host: "127.0.0.1",
    port: 1567,
    strictPort: true,
    hmr: false,
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/tests/community-dependencies/evidence/**",
      ],
    },
  },
});
