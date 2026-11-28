import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  cacheDir: "node_modules/.vite-toast-ui",
  build: {
    outDir: "node_modules/.toast-ui-dist",
    emptyOutDir: true,
    rollupOptions: { input: "tests/toast-ui/regression.html" },
  },
});
