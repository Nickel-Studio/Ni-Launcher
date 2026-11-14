/**
 * 下载中心 · 展示格式化（纯函数）。
 *
 * 字节格式化直接复用资源中心的 `formatBytes`（同属本地展示工具，避免重复实现），
 * 这里只补下载特有的速度 / 时长 / 百分比格式。
 */
import { EMPTY_VALUE, formatBytes } from "../resources/format";

export { EMPTY_VALUE, formatBytes };

/** 真实零速显示 0 B/s；未知速度与零速不同。 */
export function formatSpeed(bps: number | null): string {
  if (bps === null || !Number.isFinite(bps) || bps < 0) return EMPTY_VALUE;
  return bps === 0 ? "0 B/s" : `${formatBytes(bps)}/s`;
}

/** 毫秒 → 「3 分 12 秒」；未知或为负返回占位符。 */
export function formatDuration(ms: number | null): string {
  if (ms === null || !Number.isFinite(ms) || ms < 0) return EMPTY_VALUE;
  const totalSeconds = Math.round(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours} 小时 ${minutes} 分`;
  if (minutes > 0) return `${minutes} 分 ${seconds} 秒`;
  return `${seconds} 秒`;
}

/** 0–100 → 「42%」；未知返回占位符。 */
export function formatPercent(percent: number | null): string {
  if (percent === null || !Number.isFinite(percent)) return EMPTY_VALUE;
  return `${Math.round(Math.max(0, Math.min(100, percent)))}%`;
}

/** 总体完成度：优先后端的 percent，缺失时用「已完成文件 / 总文件」兜底。 */
export function resolvePercent(
  percent: number | null,
  filesDone: number,
  filesTotal: number
): number | null {
  if (percent !== null && Number.isFinite(percent)) {
    return Math.max(0, Math.min(100, percent));
  }
  if (filesTotal > 0) {
    return Math.max(
      0,
      Math.min(100, Math.round((filesDone / filesTotal) * 100))
    );
  }
  return null;
}
