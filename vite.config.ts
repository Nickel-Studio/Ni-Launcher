import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],
  // Keep dev optimization local and independent of fixture entry points.
  cacheDir: ".vite",
  optimizeDeps: {
    entries: ["index.html"],
    include: [
      "react",
      "react-dom",
      "react-dom/client",
      "react/jsx-runtime",
      "react/jsx-dev-runtime",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/api/window",
      "@tauri-apps/plugin-opener",
    ],
    holdUntilCrawlEnd: false,
    // 项目源码只引用下方 include 列出的依赖；关闭额外扫描，
    // 避免开发服务启动时因依赖扫描阻塞首个请求（表现为停在启动画面）。
    noDiscovery: true,
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 4. 只监视应用源码：src-tauri、测试夹具与构建产物目录体积很大，
      // 纳入监视会在开发启动时长时间占用事件循环，使首个页面请求迟迟得不到响应。
      ignored: ["**/src-tauri/**", "**/tests/**", "**/dist/**", "**/.vite/**"],
    },
  },
}));
