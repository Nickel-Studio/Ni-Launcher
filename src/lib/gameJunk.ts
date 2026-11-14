import { invoke } from "@tauri-apps/api/core";

/**
 * 清理游戏垃圾：统计与删除分离，前端先展示可释放空间再让用户确认。
 *
 * 后端只清理游戏目录的 `logs/`、`crash-reports/`、JVM / authlib 崩溃日志，
 * 以及启动器配置目录的原子写残留；不触碰游戏本体、Mod、资源包与存档。
 */
export interface GameJunkScan {
  totalBytes: number;
  fileCount: number;
}

export interface GameJunkCleanResult {
  freedBytes: number;
  removedCount: number;
  failedCount: number;
}

export async function scanGameJunk(): Promise<GameJunkScan> {
  return invoke<GameJunkScan>("game_junk_scan");
}

export async function cleanGameJunk(): Promise<GameJunkCleanResult> {
  return invoke<GameJunkCleanResult>("game_junk_clean");
}

export function describeGameJunkError(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "请稍后重试";
}
