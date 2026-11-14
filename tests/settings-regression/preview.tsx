/**
 * 浏览器预览（无 Tauri IPC）下的 GameMemory 检查：
 * 组件只应使用传入的首屏值渲染，不能因缺少 `game_runtime_info` 而每秒报错。
 * 由 run-regression.mjs 的 settings-preview 路由复用。
 */
import { createRoot } from "react-dom/client";
import { GameMemory } from "../../src/components/settings/GameMemory";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";

const OBSERVE_MS = 2600;
const POLL_MS = 200;

const root = createRoot(document.getElementById("root")!);
root.render(
  <div className="fixture">
    <GameMemory
      totalMemoryMb={16384}
      availableMemoryMb={10000}
      suggestedMemoryMb={4096}
      allocatedMemoryMb={4096}
    />
  </div>
);

const result = document.getElementById("regression-result")!;
const started = performance.now();
const timer = window.setInterval(() => {
  const error = document.querySelector(".st-game-memory__error")?.textContent ?? "";
  const segments = document.querySelectorAll(".st-game-memory__segment").length;
  if (error) {
    document.documentElement.dataset.regression = "fail";
    result.dataset.status = "fail";
    result.textContent = `浏览器预览出现轮询错误：${error}`;
    window.clearInterval(timer);
    return;
  }
  if (performance.now() - started < OBSERVE_MS) return;
  document.documentElement.dataset.regression = "pass";
  result.dataset.status = "pass";
  result.textContent = `浏览器预览无轮询错误（内存条 ${segments} 段）`;
  window.clearInterval(timer);
}, POLL_MS);
