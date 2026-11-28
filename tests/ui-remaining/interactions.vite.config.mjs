import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  cacheDir: "node_modules/.vite-interactions",
  build: {
    outDir: "node_modules/.interactions-dist",
    emptyOutDir: true,
    rollupOptions: { input: [
      "tests/ui-remaining/interactions.html",
      "tests/loading-animation/regression.html",
      "tests/community-dependencies/regression.html",
    ] },
  },
});
