import { invoke } from "@tauri-apps/api/core";
import { startManagedDownload } from "../components/download/store";
import { setLaunchInstance } from "./instances";
import { isTauri } from "./settings";

export interface PortableStatus {
  packageId: string;
  name: string;
  gameDirectory: string;
  state: "pending" | "running" | "installed";
  taskId: string | null;
  versionId: string | null;
}

export async function readPortableStatus(): Promise<PortableStatus | null> {
  if (!isTauri()) return null;
  return invoke<PortableStatus | null>("portable_bootstrap_status");
}

export async function startPortableImport(
  status: PortableStatus
): Promise<PortableStatus | null> {
  const taskId = await startManagedDownload(
    {
      kind: "modpack",
      versionId: status.name,
      instanceName: status.name,
      dir: status.gameDirectory,
      loader: "",
      loaderVersion: null,
      source: "official_auto",
    },
    () => invoke<string | null>("portable_bootstrap_start")
  );
  // 等待队列取消也会返回 null；只有后端完成记录能确认已安装。
  return taskId
    ? { ...status, taskId, state: "running" }
    : readPortableStatus();
}

let initial: Promise<PortableStatus | null> | null = null;
export function initializePortable(): Promise<PortableStatus | null> {
  initial ??= (async () => {
    let status = await readPortableStatus();
    if (status?.state === "pending") status = await startPortableImport(status);
    if (status?.state === "installed" && status.versionId) {
      setLaunchInstance({
        root: status.gameDirectory,
        versionId: status.versionId,
        displayName: status.name,
        loader: "unknown",
        icon: "grass",
      });
    }
    return status;
  })();
  return initial;
}
