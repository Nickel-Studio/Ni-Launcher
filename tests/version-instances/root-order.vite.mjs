import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const suite = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(suite, "../..");

export function rootOrderConfig(outDir, customLogger) {
  return defineConfig({
    configFile: false,
    root,
    plugins: [react()],
    publicDir: false,
    cacheDir: path.join(path.dirname(outDir), "vite-cache"),
    clearScreen: false,
    customLogger,
    build: {
      outDir,
      emptyOutDir: true,
      copyPublicDir: false,
      sourcemap: true,
      reportCompressedSize: false,
      rollupOptions: { input: path.join(suite, "root-order.html") },
    },
    preview: { host: "127.0.0.1", port: 0, strictPort: false, open: false },
  });
}
