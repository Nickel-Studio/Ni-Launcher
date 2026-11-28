import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  cacheDir: "node_modules/.vite-feature-mod-oauth-ui",
  build: {
    outDir: "node_modules/.feature-mod-oauth-ui-dist",
    emptyOutDir: true,
    rollupOptions: {
      input: [
        "tests/feature-mod-oauth-ui/regression.html",
        "tests/feature-mod-oauth-ui/home.html",
        "tests/community-dependencies/regression.html",
        "tests/community-version/regression.html",
        "tests/ui-foundation/regression.html",
        "tests/ui-remaining/regression.html",
      ],
    },
  },
  server: {
    host: "127.0.0.1",
    port: 1570,
    strictPort: true,
    hmr: false,
    watch: null,
  },
});
