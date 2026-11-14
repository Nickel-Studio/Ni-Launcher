import { invoke } from "@tauri-apps/api/core";

/** 手动优化与启动前开关独立；Windows 请求 UAC，Unix 由后端选择提权途径。 */

export type MemoryOptimizeStatus =
  | "success"
  | "permissionDenied"
  | "skipped"
  | "failed"
  | "busy";

export interface MemoryOptimizeReport {
  status: MemoryOptimizeStatus;
  /** 枚举到的进程总数（macOS / Linux 为 0：清理的是系统页缓存）。 */
  totalProcesses: number;
  succeeded: number;
  accessDenied: number;
  skipped: number;
  /** 系统可用物理内存的变化量（字节），可能为负。 */
  availableDeltaBytes: number;
  standbyPurged: boolean;
  elapsedMs: number;
  /** 实际使用的权限路径：normal / sudo / root / uac / skipped。 */
  elevation: string;
  message: string | null;
}

export interface MemoryOptimizeCapability {
  platform: "windows" | "macos" | "linux" | "other" | string;
  elevated: boolean;
  /** 需要用户输入 sudo / su（root）密码才能真正执行。 */
  needsPassword: boolean;
  hasSudo: boolean;
  /** 只有 Windows 支持清空待命内存列表（分页回收）。 */
  supportsStandbyPurge: boolean;
  available: boolean;
  message: string | null;
}

export async function fetchMemoryOptimizeCapability(): Promise<MemoryOptimizeCapability> {
  return invoke<MemoryOptimizeCapability>("memory_optimize_capability");
}

/**
 * 立即执行一次内存优化。
 * `password` 只在 macOS / Linux 需要提权时传入，后端仅写入子进程 stdin。
 */
export async function runMemoryOptimize(
  purgeStandby: boolean,
  password?: string
): Promise<MemoryOptimizeReport> {
  return invoke<MemoryOptimizeReport>("memory_optimize_run", {
    purgeStandby,
    password: password ?? null,
  });
}

/** 把 invoke 抛出的任意错误转成可读中文，供 toast 使用。 */
export function describeMemoryOptimizeError(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "请稍后重试";
}
