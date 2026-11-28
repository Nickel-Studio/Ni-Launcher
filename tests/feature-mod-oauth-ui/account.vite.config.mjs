import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  cacheDir: "node_modules/.vite-account-oauth-ui",
  build: {
    outDir: "node_modules/.account-oauth-ui-dist",
    emptyOutDir: true,
    rollupOptions: {
      input: ["tests/feature-mod-oauth-ui/account-settings.html", "tests/feature-mod-oauth-ui/home.html"],
    },
  },
});
