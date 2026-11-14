import { invoke } from "@tauri-apps/api/core";

/** 日期键使用本地时区，避免 UTC 日期与用户日历相差一天。 */
export interface JrrpReport {
  /** 1~100，同一天内恒定。 */
  value: number;
  comment: string;
}

export function localDateKey(date = new Date()): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}${month}${day}`;
}

export async function fetchJrrp(): Promise<JrrpReport> {
  return invoke<JrrpReport>("jrrp_compute", { dateKey: localDateKey() });
}

export function describeJrrpError(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "请稍后重试";
}
