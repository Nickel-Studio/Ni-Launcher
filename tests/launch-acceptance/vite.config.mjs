import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: fileURLToPath(new URL("./fixture/", import.meta.url)),
  publicDir: false,
  cacheDir: process.env.LAUNCH_FIXTURE_CACHE || fileURLToPath(new URL("./.vite/", import.meta.url)),
  plugins: [react()],
  clearScreen: false,
  define: { "process.env.NODE_ENV": JSON.stringify("development") },
  build: {
    outDir: process.env.LAUNCH_FIXTURE_OUT || fileURLToPath(new URL("./dist/", import.meta.url)),
    emptyOutDir: false,
    minify: false,
  },
});
