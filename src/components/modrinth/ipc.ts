/**
 * Modrinth 社区资源 IPC。API 与文件分别采用任务设置快照的来源策略；
 * HTTP 候选回退和校验由后端统一处理。
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import { loadInstanceSearchFilter } from "../resources/ipc";
import { normalizeSearch, normalizeVersions } from "./parse";
import type {
  ModrinthFilter,
  ModrinthKind,
  ModrinthSearchResult,
  ModrinthVersion,
} from "./types";

/** 检索一页结果。 */
export async function searchModrinth(params: {
  kind: ModrinthKind;
  query: string;
  gameVersion: string;
  loader: string;
  offset: number;
  limit: number;
}): Promise<ModrinthSearchResult> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("modrinth_search", {
    kind: params.kind,
    query: params.query,
    gameVersion: params.gameVersion,
    loader: params.loader,
    offset: params.offset,
    limit: params.limit,
  });
  return normalizeSearch(raw, "modrinth");
}

/** 列出某项目的版本（按 MC 版本与加载器过滤由后端完成）。 */
export async function projectVersions(params: {
  kind: ModrinthKind;
  slugOrId: string;
  gameVersion: string;
  loader: string;
}): Promise<ModrinthVersion[]> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("modrinth_project_versions", {
    kind: params.kind,
    slugOrId: params.slugOrId,
    gameVersion: params.gameVersion,
    loader: params.loader,
  });
  return normalizeVersions(raw);
}

/** 安装资源主文件到实例对应目录；返回 taskId，进度经 `install-event` 推送。 */
export async function installModrinth(params: {
  kind: ModrinthKind;
  slugOrId: string;
  versionId: string;
  dir: string;
}): Promise<string> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("modrinth_install", {
    kind: params.kind,
    slugOrId: params.slugOrId,
    versionId: params.versionId,
    dir: params.dir,
  });
  return typeof raw === "string" ? raw.trim() : "";
}

/**
 * 另存为资源主文件：后端先解析元数据再弹原生保存对话框（默认目录 = 实例
 * mods，LiteLoader 为 mods/<mc>），确认后创建受管下载任务。
 * 返回 taskId；取消对话框返回 null（不创建任务）。
 */
export async function saveAsModrinth(params: {
  kind: ModrinthKind;
  slugOrId: string;
  versionId: string;
  root: string;
  instanceVersionId: string;
  dir: string;
  source?: string;
}): Promise<string | null> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string | null>("modrinth_save_as", {
    kind: params.kind,
    slugOrId: params.slugOrId,
    versionId: params.versionId,
    root: params.root,
    instanceVersionId: params.instanceVersionId,
    dir: params.dir,
    source: params.source ?? null,
  });
  return typeof raw === "string" ? raw.trim() : null;
}

/** 用系统默认浏览器打开 Modrinth 项目页。 */
export async function openModrinthProject(slug: string): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("modrinth_open_project", { slug });
}

/** 只匹配首页选定的 root + versionId；未选择或不存在时不猜测安装目标。 */
export function loadModrinthFilter(): Promise<ModrinthFilter | null> {
  return loadInstanceSearchFilter();
}
