import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const fixtureRoot = fileURLToPath(new URL("./fixture/", import.meta.url));
const projectRoot = fileURLToPath(new URL("../../", import.meta.url));

export default defineConfig({
  root: fixtureRoot,
  publicDir: false,
  cacheDir: fileURLToPath(new URL("./.vite/", import.meta.url)),
  plugins: [react()],
  clearScreen: false,
  define: { "process.env.NODE_ENV": JSON.stringify("development") },
  server: {
    host: "127.0.0.1",
    hmr: false,
    watch: null,
    fs: { allow: [projectRoot] },
  },
  optimizeDeps: {
    entries: [fileURLToPath(new URL("./fixture/index.html", import.meta.url))],
    include: ["react", "react/jsx-runtime", "react/jsx-dev-runtime", "react-dom/client"],
    noDiscovery: true,
  },
  build: {
    outDir: process.env.STARTUP_FIXTURE_OUT || fileURLToPath(new URL("./dist/", import.meta.url)),
    emptyOutDir: false,
    minify: false,
  },
});
