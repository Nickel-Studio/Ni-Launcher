/**
 * CurseForge 社区资源 IPC。API 与文件分别采用任务设置快照的来源策略；
 * 无官方 Key 时仍可使用镜像，Key 仅由后端发送给官方候选。
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import { normalizeSearch, normalizeVersionPage } from "../modrinth/parse";
import type {
  CurseforgeFilesParams,
  CurseforgeInstallParams,
  CurseforgeSearchParams,
  CurseforgeKind,
} from "./types";

/** CurseForge 引擎是否已配置 API Key（key 本身不回传前端）。 */
export async function fetchCurseforgeConfigured(): Promise<boolean> {
  if (!isTauri()) return false;
  const raw = await invoke<string>("cf_config_status");
  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && "configured" in parsed) {
      return (parsed as { configured: unknown }).configured === true;
    }
  } catch {
    /* 后端契约是 JSON 文本；解析失败按未配置处理 */
  }
  return false;
}

/** 检索一页结果（返回值与 `searchModrinth` 同形）。 */
export async function searchCurseforge(
  params: CurseforgeSearchParams
): Promise<{ total: number; hits: import("../modrinth/types").ModrinthHit[] }> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("curseforge_search", {
    kind: params.kind,
    query: params.query,
    gameVersion: params.gameVersion,
    loader: params.loader,
    offset: params.offset,
    limit: params.limit,
  });
  return normalizeSearch(raw, "curseforge");
}

/** 列出某项目的文件（按 MC 版本与加载器过滤由后端完成）。 */
export async function curseforgeProjectFiles(
  params: CurseforgeFilesParams
): Promise<import("../modrinth/types").ProjectVersionPage> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("curseforge_project_files", {
    kind: params.kind,
    projectId: params.projectId,
    gameVersion: params.gameVersion,
    loader: params.loader,
    offset: params.offset ?? 0,
    limit: params.limit ?? 50,
  });
  return normalizeVersionPage(raw);
}

/** 安装指定文件到实例对应目录；返回 taskId，进度经 `install-event` 推送。 */
export async function installCurseforge(
  params: CurseforgeInstallParams
): Promise<string> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("curseforge_install", {
    kind: params.kind,
    projectId: params.projectId,
    fileId: params.fileId,
    dir: params.dir,
  });
  return typeof raw === "string" ? raw.trim() : "";
}

/**
 * 另存为指定文件：后端先解析元数据再弹原生保存对话框（默认目录 = 实例
 * mods，LiteLoader 为 mods/<mc>），确认后创建受管下载任务。
 * 返回 taskId；取消对话框返回 null（不创建任务）。
 */
export async function saveAsCurseforge(params: {
  kind: CurseforgeKind;
  projectId: string;
  fileId: string;
  root: string;
  instanceVersionId: string;
  dir: string;
  source?: string;
}): Promise<string | null> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string | null>("curseforge_save_as", {
    kind: params.kind,
    projectId: params.projectId,
    fileId: params.fileId,
    root: params.root,
    instanceVersionId: params.instanceVersionId,
    dir: params.dir,
    source: params.source ?? null,
  });
  return typeof raw === "string" ? raw.trim() : null;
}

/** 用系统默认浏览器打开 CurseForge 项目页。 */
export async function openCurseforgeProject(
  kind: CurseforgeKind,
  slug: string
): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("curseforge_open_project", { kind, slug });
}
