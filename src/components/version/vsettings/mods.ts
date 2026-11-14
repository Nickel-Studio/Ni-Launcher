import { invoke } from "@tauri-apps/api/core";
import { isTauri, type SourcePolicy } from "../../../lib/settings";
import { startManagedDownload } from "../../download/store";

export interface ModEntry {
  path: string;
  physicalPaths: string[];
  name: string;
  description: string;
  version: string;
  enabled: boolean;
  updateAvailable: boolean;
  /** JAR 内图标条目名；图标字节由 `version_mod_icon` 按需读取。 */
  iconPath: string | null;
  duplicateGroup: string | null;
  tags: string[];
  /** 真实元数据是否已到达；仅用于行内淡入过渡，占位阶段为 false。 */
  metaLoaded: boolean;
}

/** 单条 Mod 的真实元数据；失败时后端返回 null，前端继续保留占位。 */
export interface ModMeta {
  name: string;
  description: string;
  version: string;
  tags: string[];
  iconPath: string | null;
  duplicateGroup: string | null;
}

/** 更新检查的逐项结果；只合并对应行，不重新发送整张列表。 */
export interface ModUpdateState {
  path: string;
  updateAvailable: boolean;
}

export type ModAction =
  | "enable"
  | "disable"
  | "delete"
  | "copy"
  | "install"
  | "update";

export function modFileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** 去掉状态与容器后缀，得到首屏展示名：`foo-1.2.jar.disabled` → `foo-1.2`。 */
export function modDisplayName(path: string): string {
  const file = modFileName(path);
  const name = file
    .replace(/\.(disabled|old)$/iu, "")
    .replace(/\.(jar|zip|litemod)$/iu, "");
  return name || file;
}

export function normalizeMods(raw: unknown): ModEntry[] {
  const value: unknown = typeof raw === "string" ? JSON.parse(raw) : raw;
  if (!Array.isArray(value))
    throw new Error("Mod 列表格式无法识别，请重新扫描");
  return value.map((item: unknown) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new Error("Mod 条目格式无法识别，请重新扫描");
    }
    const entry = item as Record<string, unknown>;
    if (
      typeof entry.path !== "string" ||
      !entry.path ||
      typeof entry.enabled !== "boolean"
    ) {
      throw new Error("Mod 文件信息不完整，请重新扫描");
    }
    const physicalPaths = Array.isArray(entry.physicalPaths)
      ? [
          ...new Set(
            entry.physicalPaths.filter(
              (path: unknown): path is string =>
                typeof path === "string" && path.length > 0
            )
          ),
        ]
      : [];
    const tags = Array.isArray(entry.tags)
      ? [
          ...new Set(
            entry.tags.filter(
              (tag: unknown): tag is string =>
                typeof tag === "string" && tag.trim().length > 0
            )
          ),
        ]
      : [];
    return {
      path: entry.path,
      physicalPaths: physicalPaths.length > 0 ? physicalPaths : [entry.path],
      name:
        typeof entry.name === "string" && entry.name
          ? entry.name
          : modDisplayName(entry.path),
      description:
        typeof entry.description === "string"
          ? entry.description
          : modDisplayName(entry.path),
      version: typeof entry.version === "string" ? entry.version : "",
      enabled: entry.enabled,
      updateAvailable: entry.updateAvailable === true,
      iconPath:
        typeof entry.iconPath === "string" && entry.iconPath
          ? entry.iconPath
          : null,
      duplicateGroup:
        typeof entry.duplicateGroup === "string" && entry.duplicateGroup
          ? entry.duplicateGroup
          : null,
      tags,
      metaLoaded: entry.metaLoaded === true,
    };
  });
}

/** 元数据可能缺少可选字段；名称是唯一必需项，其余安全回退为空值。 */
export function normalizeModMeta(raw: unknown): ModMeta | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const value = raw as Record<string, unknown>;
  if (typeof value.name !== "string" || !value.name) return null;
  return {
    name: value.name,
    description: typeof value.description === "string" ? value.description : "",
    version: typeof value.version === "string" ? value.version : "",
    tags: Array.isArray(value.tags)
      ? [
          ...new Set(
            value.tags.filter(
              (tag): tag is string =>
                typeof tag === "string" && tag.trim().length > 0
            )
          ),
        ]
      : [],
    iconPath:
      typeof value.iconPath === "string" && value.iconPath
        ? value.iconPath
        : null,
    duplicateGroup:
      typeof value.duplicateGroup === "string" && value.duplicateGroup
        ? value.duplicateGroup
        : null,
  };
}

/** 更新结果只接受带路径的条目，避免坏响应覆盖正确行。 */
export function normalizeModUpdates(raw: unknown): ModUpdateState[] {
  if (!Array.isArray(raw)) {
    throw new Error("Mod 更新结果格式无法识别，请重新扫描");
  }
  return raw.flatMap((item: unknown) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) return [];
    const value = item as Record<string, unknown>;
    if (typeof value.path !== "string" || !value.path) return [];
    return [
      {
        path: value.path,
        updateAvailable: value.updateAvailable === true,
      },
    ];
  });
}

export async function fetchVersionMods(
  dir: string,
  versionId: string,
  checkUpdates = true
): Promise<ModEntry[]> {
  if (!isTauri()) throw new Error("请在启动器中打开 Mod 管理");
  return normalizeMods(
    await invoke<unknown>("version_mods", { dir, versionId, checkUpdates })
  );
}

/** 首屏列表：只读目录与文件名占位，正常情况下远快于完整元数据扫描。 */
export async function fetchVersionModList(
  dir: string,
  versionId: string
): Promise<ModEntry[]> {
  if (!isTauri()) throw new Error("请在启动器中打开 Mod 管理");
  return normalizeMods(
    await invoke<unknown>("version_mod_list", { dir, versionId })
  );
}

/** 单条真实元数据；null 表示该 Mod 没有可用清单，前端保留占位。 */
export async function fetchVersionModMeta(
  dir: string,
  versionId: string,
  path: string,
  physicalPaths: string[]
): Promise<ModMeta | null> {
  if (!isTauri()) return null;
  const raw = await invoke<unknown>("version_mod_meta", {
    dir,
    versionId,
    path,
    physicalPaths,
  });
  return normalizeModMeta(raw);
}

/** 只对已加载路径做更新检查；逐项结果由调用方按 path 合并。 */
export async function fetchVersionModUpdates(
  dir: string,
  versionId: string,
  paths: string[]
): Promise<ModUpdateState[]> {
  if (!isTauri()) return [];
  return normalizeModUpdates(
    await invoke<unknown>("version_mod_updates", { dir, versionId, paths })
  );
}

/**
 * 按需读取单个 Mod 图标（JAR 内条目 → dataURL）。
 * 图标与列表分离：失败只回退通用图标，不影响扫描、启停、删除或安装。
 */
export async function fetchVersionModIcon(
  dir: string,
  versionId: string,
  path: string,
  iconEntry: string
): Promise<string | null> {
  if (!isTauri()) return null;
  const raw = await invoke<unknown>("version_mod_icon", {
    dir,
    versionId,
    path,
    iconEntry,
  });
  return typeof raw === "string" && raw.startsWith("data:image/") ? raw : null;
}

/** update 的 taskId 接入统一下载；其他动作的 null 只代表命令结束或取消选择。 */
export async function actOnVersionMods(
  dir: string,
  versionId: string,
  paths: string[],
  action: ModAction,
  source: SourcePolicy,
  displayName: string
): Promise<string | null> {
  if (!isTauri()) throw new Error("请在启动器中管理 Mod 文件");
  const run = () =>
    invoke<string | null>("version_mods_action", {
      dir,
      versionId,
      paths,
      action,
    });
  if (action !== "update") return run();
  return startManagedDownload(
    {
      dir,
      versionId,
      instanceName: `${displayName} · Mod 更新`,
      loader: "",
      loaderVersion: null,
      source,
      kind: "resource",
    },
    run
  );
}

/** 同一层优先使用新字段；null 表示继续继承，而不是关闭。 */
export function modUpdatesSetting(
  record: Record<string, unknown>
): boolean | null {
  for (const key of ["disableModUpdates", "noModUpdates"]) {
    if (typeof record[key] === "boolean") return record[key];
  }
  return null;
}

/** 设置页与 Mod 操作使用同一读取规则；本地 false 必须能覆盖全局 true。 */
export function modUpdatesDisabled(
  local: Record<string, unknown>,
  global: Record<string, unknown>
): boolean {
  return modUpdatesSetting(local) ?? modUpdatesSetting(global) ?? false;
}
