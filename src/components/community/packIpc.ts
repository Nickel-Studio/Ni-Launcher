import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import type { HitSource } from "../modrinth/types";

export interface PackRequest {
  platform: HitSource;
  projectId: string;
  versionId: string;
  instanceName?: string;
}

export async function installModpack(request: PackRequest): Promise<string> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  try {
    return await invoke<string>("modpack_install", { request });
  } catch (error) {
    if (
      error &&
      typeof error === "object" &&
      "message" in error &&
      typeof error.message === "string"
    ) {
      throw new Error(error.message);
    }
    throw new Error(String(error));
  }
}
