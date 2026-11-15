import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

export default defineConfig({
  root: fileURLToPath(new URL("../../", import.meta.url)),
  cacheDir: fileURLToPath(new URL("./.vite", import.meta.url)),
  plugins: [react()],
  clearScreen: false,
  define: { "process.env.NODE_ENV": JSON.stringify("development") },
  build: {
    outDir: process.env.THEME_FIXTURE_OUT ?? fileURLToPath(new URL("./browser-dist", import.meta.url)),
    emptyOutDir: false,
    minify: false,
    rolldownOptions: {
      input: {
        app: fileURLToPath(new URL("./fixtures/app.html", import.meta.url)),
        runtime: fileURLToPath(new URL("./fixtures/runtime.html", import.meta.url)),
      },
    },
  },
  server: { host: "127.0.0.1", hmr: false },
  optimizeDeps: {
    entries: ["tests/theme-acceptance/fixtures/app.html", "tests/theme-acceptance/fixtures/runtime.html"],
    include: ["react", "react/jsx-runtime", "react/jsx-dev-runtime", "react-dom/client"],
    noDiscovery: true,
  },
});
