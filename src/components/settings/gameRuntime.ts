import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import { PRODUCT_NAME } from "../../lib/brand";

export interface GameRuntimeInfo {
  totalMemoryMb: number | null;
  availableMemoryMb: number | null;
  suggestedMemoryMb: number | null;
  capabilities: Record<string, boolean>;
}

function memoryValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

/** 接受 Tauri 对象或 JSON 文本；平台能力由后端实时返回。 */
export async function loadGameRuntimeInfo(): Promise<GameRuntimeInfo> {
  if (!isTauri()) throw new Error("请在桌面启动器中刷新运行信息");
  const result = await invoke<unknown>("game_runtime_info");
  const raw: unknown = typeof result === "string" ? JSON.parse(result) : result;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error(`运行信息格式无效，请重新启动 ${PRODUCT_NAME} 后重试`);
  }
  const record = raw as Record<string, unknown>;
  if (!record.capabilities || typeof record.capabilities !== "object" || Array.isArray(record.capabilities)) {
    throw new Error(`运行能力信息缺失，请重新启动 ${PRODUCT_NAME} 后重试`);
  }
  const capabilities: Record<string, boolean> = {};
  for (const [name, supported] of Object.entries(record.capabilities)) {
    if (typeof supported === "boolean") capabilities[name] = supported;
  }
  return {
    totalMemoryMb: memoryValue(record.totalMemoryMb),
    availableMemoryMb: memoryValue(record.availableMemoryMb),
    suggestedMemoryMb: memoryValue(record.suggestedMemoryMb),
    capabilities,
  };
}
