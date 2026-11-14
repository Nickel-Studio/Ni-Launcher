/**
 * 资源中心 · 与 Rust 后端的全部 IPC 通道。
 *
 * 集中在单个模块的理由：这些命令由后端并行实现，缺失或失败时必须整体可降级，
 * 因此调用点只负责「调用 + 归一化 + 抛出可展示的错误」，
 * 由上层（useResources / ResourceCard）决定如何呈现，绝不让界面白屏。
 */

import { invoke } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { getLaunchInstance, type VersionFocus } from "../../lib/instances";
import { isTauri } from "../../lib/settings";
import {
  baseName,
  directoryOf,
  errorMessage,
  normalizeEntries,
  normalizeInstances,
} from "./parse";
import type {
  InstanceInfo,
  InstanceRoot,
  InstanceSearchFilter,
  ResourceEntry,
  ResourceInstance,
  ResourceKind,
} from "./types";

/** 扫描单个种类目录，返回归一化条目；解析失败会抛错，由调用方降级。 */
export async function scanKind(
  dir: string,
  kind: ResourceKind
): Promise<ResourceEntry[]> {
  const raw = await invoke<string>("resources_scan", { dir, kind });
  return normalizeEntries(raw, kind);
}

/** 从 `instances_get` 的结果里挑出当前激活实例。 */
function pickActive(info: InstanceInfo | null): InstanceRoot | null {
  if (!info) return null;
  if (info.active) {
    const match = info.roots.find((root) => root.dir === info.active);
    return { dir: info.active, label: match?.label ?? baseName(info.active) };
  }
  return info.roots[0] ?? null;
}

/**
 * 解析当前 `.minecraft` 根目录。
 *
 * 首选 `instances_get` 的 active 实例；该命令缺失时退回已存在的
 * `default_minecraft_dir`；两者都拿不到则返回 null，
 * 上层据此展示「尚未检测到实例目录」空状态并给出可执行提示。
 */
export async function resolveInstanceRoot(): Promise<InstanceRoot | null> {
  if (!isTauri()) return null;

  try {
    const raw = await invoke<string>("instances_get");
    const active = pickActive(normalizeInstances(raw));
    if (active) return active;
  } catch {
    /* 命令缺失或调用失败：继续尝试默认目录 */
  }

  try {
    const raw = await invoke<string>("default_minecraft_dir");
    const dir = typeof raw === "string" ? raw.trim() : "";
    if (dir) return { dir, label: baseName(dir) };
  } catch {
    /* 两条路径都不可用：交由上层展示空状态 */
  }

  return null;
}

/**
 * 打开条目所在文件夹。
 *
 * 首选 `openPath(所在目录)`；该命令需要 `opener:allow-open-path` 权限，
 * 未授权时退回 `revealItemInDir`（opener 默认权限已包含），
 * 在文件管理器中定位条目。两者都失败时抛出可展示的中文错误。
 */
export async function openContainingFolder(
  entry: ResourceEntry
): Promise<void> {
  const dir = directoryOf(entry.path);
  const attempts: Array<() => Promise<void>> = [];
  if (dir) attempts.push(() => openPath(dir));
  attempts.push(() => revealItemInDir(entry.path));

  let lastMessage = "";
  for (const attempt of attempts) {
    try {
      await attempt();
      return;
    } catch (err) {
      lastMessage = errorMessage(err);
    }
  }
  throw new Error(lastMessage);
}

interface ResourceInstanceContext {
  resourceDir: string;
  displayName: string;
  minecraftVersion: string | null;
  loader: string;
}

/** 后端同时解析真实资源目录与兼容信息；显式目标失效时不改用首页实例。 */
export async function loadResourceInstance(
  target?: VersionFocus
): Promise<ResourceInstance | null> {
  const selected = target ?? getLaunchInstance();
  if (!isTauri() || !selected) return null;
  const context = await invoke<ResourceInstanceContext | null>(
    "resource_instance_context",
    { dir: selected.root, versionId: selected.versionId }
  );
  if (
    !context ||
    typeof context.resourceDir !== "string" ||
    !context.resourceDir.trim()
  )
    return null;
  return {
    root: selected.root,
    versionId: selected.versionId,
    dir: context.resourceDir,
    label: typeof context.displayName === "string" ? context.displayName : "",
    mcVersion:
      typeof context.minecraftVersion === "string"
        ? context.minecraftVersion.trim()
        : "",
    loader: typeof context.loader === "string" ? context.loader : "",
  };
}

/** 关闭隔离时不同实例可共用资源目录，安装前仍需核对身份与真实游戏版本。 */
export async function revalidateResourceInstance(
  expected: ResourceInstance,
  target?: VersionFocus
): Promise<ResourceInstance> {
  const current = await loadResourceInstance(target);
  const selected = target ?? getLaunchInstance();
  if (
    !expected.dir ||
    !current ||
    !selected ||
    current.root !== expected.root ||
    current.versionId !== expected.versionId ||
    selected.root !== current.root ||
    selected.versionId !== current.versionId ||
    current.dir !== expected.dir ||
    current.mcVersion !== expected.mcVersion ||
    current.loader !== expected.loader
  ) {
    throw new Error("当前实例或游戏版本已变化，请返回资源列表后重新打开项目。");
  }
  if (!current.mcVersion) {
    throw new Error(
      "无法识别实例的 Minecraft 版本，请先在实例设置中补全文件，再返回安装。"
    );
  }
  return current;
}

export async function loadInstanceSearchFilter(
  target?: VersionFocus
): Promise<InstanceSearchFilter | null> {
  return loadResourceInstance(target);
}

/** 资源目录服从有效版本隔离设置，与游戏安装根目录区分。 */
export async function resolveResourceRoot(
  target?: VersionFocus
): Promise<InstanceRoot | null> {
  const resolved = await loadResourceInstance(target);
  if (target && !resolved) {
    throw new Error("当前实例已不存在，请返回版本列表重新选择。");
  }
  return resolved;
}
