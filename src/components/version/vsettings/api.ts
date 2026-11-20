/**
 * 版本设置页 · 共享类型与后端 IPC。
 *
 * 后端全部以「JSON 文本」返回（与 instances 模块同一约定），这里统一做：
 * 安全解析 → 类型收敛 → 可展示的中文错误。调用点只负责「调用 + 归一化」，
 * 失败时抛出错误由上层决定降级呈现，绝不让界面白屏。
 *
 * 契约（与 Rust `version_settings.rs` 一字不差）。
 */
import { invoke } from "@tauri-apps/api/core";
import { PRODUCT_NAME } from "../../../lib/brand";
import {
  isTauri,
  normalizeSettings as normalizeAppSettings,
} from "../../../lib/settings";
import { startManagedDownload } from "../../download/store";
import {
  getLaunchInstance,
  getVersionFocus,
  setLaunchInstance,
  setVersionFocus,
} from "../../../lib/instances";
import {
  GAME_MEMORY_MIN_MB,
  GAME_MEMORY_MAX_MB,
  type GameSettings,
} from "../gameSettings";

/** 版本详情（`version_detail` 的返回结构）。 */
export interface VersionDetail {
  id: string;
  /** 实际 Minecraft 版本；命名实例从 nickelProfile/inheritsFrom 归纳。 */
  minecraftVersion: string | null;
  dir: string;
  type: string;
  loader: string;
  loaderVersion: string | null;
  icon: string;
  name: string;
  description: string;
  favorite: boolean;
  category: string;
  hasJar: boolean;
  jarSizeBytes: number;
  releaseTime: string | null;
}

/** 内存分配模式。 */
export type MemoryMode = "global" | "auto" | "custom";

export type ServerLogin = "any" | "offline" | "microsoft" | "thirdParty";

/** per-version 设置；null / global 继承，空字符串显式清空。 */
export interface VersionSettings extends Record<string, unknown> {
  versionIsolation: boolean | null;
  windowTitle: string | null;
  customInfo: string | null;
  javaPath: string | null;
  launcherVisibility: GameSettings["launcherVisibility"] | null;
  processPriority: GameSettings["processPriority"] | null;
  windowMode: GameSettings["windowMode"] | null;
  windowWidth: number | null;
  windowHeight: number | null;
  memoryMode: MemoryMode;
  memoryMb: number | null;
  memoryManagement: GameSettings["memoryManagement"] | null;
  highPerformanceGpu: boolean | null;
  offlineSkin: GameSettings["offlineSkin"] | null;
  offlineSkinName: string | null;
  offlineSkinPath: string | null;
  offlineSkinSlim: boolean | null;
  serverLogin: ServerLogin | null;
  authServer: string | null;
  serverRegistration: string | null;
  serverName: string | null;
  serverAddress: string | null;
  jvmArgs: string | null;
  gameArgs: string | null;
  preLaunchCommand: string | null;
  disableModUpdates: boolean | null;
  disableFileCheck: boolean | null;
  disableWrapper: boolean | null;
  disableLwjglAgent: boolean | null;
  /** 图标键或后端已复制的图片原路径；null 自动选择图标。 */
  icon: string | null;
}

/** Java 探测条目（`java_detect` 的返回结构）。 */
export interface JavaInstallation {
  path: string;
  version: string;
  vendor: string;
  /** 探测顺序不是实际自动选择；请调用 `java_resolve` 获取真实选择。 */
  isDefault: boolean;
  major: number;
  bits: number;
  architecture: string;
  executable: boolean;
}

export type JavaSelectionSource = "auto" | "manual" | "unavailable";

/** `java_resolve` 返回的版本要求。 */
export interface JavaRequirement {
  minMajor: number;
  maxMajor: number | null;
  preferredMajor: number;
  reason: string;
}

/** 实际会消费的 Java；`needsDownload` 为真时会由核心下载运行时。 */
export interface JavaSelection {
  path: string | null;
  source: JavaSelectionSource;
  version: string;
  vendor: string;
  major: number | null;
  bits: number | null;
  compatible: boolean;
  needsDownload: boolean;
  reason: string;
  requirement: JavaRequirement;
}

export interface JavaResolveArgs {
  mcVersion: string;
  loader: string | null;
  loaderVersion: string | null;
  releaseTime: string | null;
  javaPath: string | null;
}

/** 与后端 `resolved_memory_plan` 一致；建议值由后端自动内存算法生成。 */
export interface MemoryPlanInfo {
  totalMemoryMb: number | null;
  availableMemoryMb: number | null;
  suggestedMemoryMb: number;
  modCount: number;
}

function runtimeMemory(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.round(value)
    : null;
}

/** 读取指定实例的实际自动内存规划；未选择实例时由后端按普通实例估算。 */
export async function resolveMemoryPlan(
  gameDir: string | null,
  versionId: string | null
): Promise<MemoryPlanInfo> {
  if (!isTauri()) throw new Error("请在桌面启动器中读取内存规划");
  const raw = toData(
    await invoke<unknown>("resolved_memory_plan", { gameDir, versionId })
  );
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error(`内存规划格式无效，请重新启动 ${PRODUCT_NAME} 后重试`);
  }
  const record = raw as Record<string, unknown>;
  const suggestedMemoryMb = runtimeMemory(record.suggestedMemoryMb);
  if (suggestedMemoryMb === null || suggestedMemoryMb <= 0) {
    throw new Error(`内存规划缺少建议值，请重新启动 ${PRODUCT_NAME} 后重试`);
  }
  return {
    totalMemoryMb: runtimeMemory(record.totalMemoryMb),
    availableMemoryMb: runtimeMemory(record.availableMemoryMb),
    suggestedMemoryMb,
    modCount: runtimeMemory(record.modCount) ?? 0,
  };
}

export const DEFAULT_SETTINGS: VersionSettings = {
  versionIsolation: null,
  windowTitle: null,
  customInfo: null,
  javaPath: null,
  launcherVisibility: null,
  processPriority: null,
  windowMode: null,
  windowWidth: null,
  windowHeight: null,
  memoryMode: "global",
  memoryMb: null,
  memoryManagement: null,
  highPerformanceGpu: null,
  offlineSkin: null,
  offlineSkinName: null,
  offlineSkinPath: null,
  offlineSkinSlim: null,
  serverLogin: null,
  authServer: null,
  serverRegistration: null,
  serverName: null,
  serverAddress: null,
  jvmArgs: null,
  gameArgs: null,
  preLaunchCommand: null,
  disableModUpdates: null,
  disableFileCheck: null,
  disableWrapper: null,
  disableLwjglAgent: null,
  icon: null,
};

/** 实例与全局使用同一内存边界；物理内存由运行信息组件展示。 */
export const MEMORY_MIN_MB = GAME_MEMORY_MIN_MB;
export const MEMORY_MAX_MB = GAME_MEMORY_MAX_MB;
export const MEMORY_STEP_MB = 512;

/** 尝试解析 JSON 文本；失败返回 null，不抛错。 */
function tryParse(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

/** 命令返回值既可能是 JSON 文本，也可能已是对象 —— 统一成 unknown 再收敛。 */
function toData(raw: unknown): unknown {
  return typeof raw === "string" ? tryParse(raw) : raw;
}

/** 可选字符串：字符串且非空 → trim 后的值；否则 null。 */
function optionalString(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed || null;
}

/** Java 主版本 / 位数允许 0 表示未探测，但拒绝负数与小数。 */
function javaNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.round(value)
    : null;
}

/** 归一化版本详情（缺字段 / 形态不符时给出可渲染的兜底值）。 */
export function normalizeDetail(raw: unknown): VersionDetail {
  const data = toData(raw);
  const record =
    data && typeof data === "object" ? (data as Record<string, unknown>) : {};
  return {
    id: typeof record.id === "string" ? record.id : "",
    minecraftVersion: optionalString(record.minecraftVersion),
    dir: typeof record.dir === "string" ? record.dir : "",
    type: typeof record.type === "string" ? record.type : "unknown",
    loader: typeof record.loader === "string" ? record.loader : "unknown",
    loaderVersion: optionalString(record.loaderVersion),
    icon:
      typeof record.icon === "string" && record.icon
        ? record.icon
        : "redstone_block",
    name: typeof record.name === "string" ? record.name : "",
    description:
      typeof record.description === "string" ? record.description : "",
    favorite: record.favorite === true,
    category: typeof record.category === "string" ? record.category : "",
    hasJar: record.hasJar === true,
    jarSizeBytes:
      typeof record.jarSizeBytes === "number" ? record.jarSizeBytes : 0,
    releaseTime: optionalString(record.releaseTime),
  };
}

const MEMORY_MODES: readonly MemoryMode[] = ["global", "auto", "custom"];
const VISIBILITY_VALUES = [
  "keep",
  "hide",
  "hide-close",
  "close",
  "minimize",
] as const;
const PRIORITY_VALUES = ["low", "normal", "high"] as const;
const WINDOW_MODE_VALUES = [
  "default",
  "custom",
  "fullscreen",
  "same-as-launcher",
  "maximize",
] as const;
const MEMORY_MANAGEMENT_VALUES = [
  "auto-zgc",
  "generational-zgc",
  "g1",
  "g1-tuned",
  "custom",
] as const;
const OFFLINE_SKIN_VALUES = [
  "random",
  "steve",
  "alex",
  "official",
  "custom",
] as const;

function oneOf<T extends string>(
  value: unknown,
  choices: readonly T[]
): T | null {
  return typeof value === "string" && choices.includes(value as T)
    ? (value as T)
    : null;
}

function settingNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.round(value)
    : null;
}

/** 设置值不能 trim 或用 truthy 合并：空串也是有效覆盖。 */
function settingString(value: unknown): string | null {
  return typeof value === "string" && value !== "global" ? value : null;
}

function settingBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

/** 归一化只修正类型，不把继承或显式清空转换成默认覆盖。 */
export function normalizeSettings(raw: unknown): VersionSettings {
  const data = toData(raw);
  const record =
    data && typeof data === "object" && !Array.isArray(data)
      ? (data as Record<string, unknown>)
      : {};
  const memoryMode = MEMORY_MODES.includes(record.memoryMode as MemoryMode)
    ? (record.memoryMode as MemoryMode)
    : "global";
  const memoryMb =
    typeof record.memoryMb === "number" && Number.isFinite(record.memoryMb)
      ? Math.round(record.memoryMb)
      : null;
  return {
    ...record,
    versionIsolation: settingBoolean(record.versionIsolation),
    windowTitle: settingString(record.windowTitle),
    customInfo: settingString(record.customInfo),
    javaPath: settingString(record.javaPath),
    launcherVisibility: oneOf(record.launcherVisibility, VISIBILITY_VALUES),
    processPriority: oneOf(record.processPriority, PRIORITY_VALUES),
    windowMode: oneOf(record.windowMode, WINDOW_MODE_VALUES),
    windowWidth: settingNumber(record.windowWidth),
    windowHeight: settingNumber(record.windowHeight),
    memoryMode,
    memoryMb,
    memoryManagement: oneOf(record.memoryManagement, MEMORY_MANAGEMENT_VALUES),
    highPerformanceGpu: settingBoolean(record.highPerformanceGpu),
    offlineSkin: oneOf(record.offlineSkin, OFFLINE_SKIN_VALUES),
    offlineSkinName: settingString(record.offlineSkinName),
    offlineSkinPath: settingString(record.offlineSkinPath),
    offlineSkinSlim: settingBoolean(record.offlineSkinSlim),
    serverLogin:
      record.serverLogin === "any" ||
      record.serverLogin === "offline" ||
      record.serverLogin === "microsoft" ||
      record.serverLogin === "thirdParty"
        ? record.serverLogin
        : null,
    authServer: settingString(record.authServer),
    serverRegistration: settingString(record.serverRegistration),
    serverName: settingString(record.serverName),
    serverAddress: settingString(record.serverAddress),
    jvmArgs: settingString(record.jvmArgs),
    gameArgs: settingString(record.gameArgs),
    preLaunchCommand: settingString(record.preLaunchCommand),
    disableModUpdates: settingBoolean(record.disableModUpdates),
    disableFileCheck: settingBoolean(record.disableFileCheck),
    disableWrapper: settingBoolean(record.disableWrapper),
    disableLwjglAgent: settingBoolean(record.disableLwjglAgent),
    icon: settingString(record.icon),
  };
}

/** 读取版本详情。 */
export async function fetchVersionDetail(
  dir: string,
  versionId: string
): Promise<VersionDetail> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  return normalizeDetail(await invoke("version_detail", { dir, versionId }));
}

const settingsQueues = new Map<string, Promise<void>>();

/** 读取失败时不生成可编辑默认值；等待同一实例的排队写入完成。 */
export async function fetchVersionSettings(
  dir: string,
  versionId: string
): Promise<VersionSettings> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await settingsQueues
    .get(JSON.stringify([dir, versionId]))
    ?.catch(() => undefined);
  const data = toData(await invoke("version_settings_get", { dir, versionId }));
  if (!data || typeof data !== "object" || Array.isArray(data)) {
    throw new Error("实例设置格式无法识别，请重新读取后重试");
  }
  return normalizeSettings(data);
}

/** 保存 per-version 设置（整份写回，后端做大小与合法性校验）。 */
export async function saveVersionSettings(
  dir: string,
  versionId: string,
  settings: VersionSettings
): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const key = JSON.stringify([dir, versionId]);
  const json = JSON.stringify(settings);
  const previous = settingsQueues.get(key) ?? Promise.resolve();
  const pending = previous
    .catch(() => undefined)
    .then(async () => {
      await invoke("version_settings_set", { dir, versionId, json });
    });
  settingsQueues.set(key, pending);
  try {
    await pending;
  } finally {
    if (settingsQueues.get(key) === pending) settingsQueues.delete(key);
  }
}

/** 修改版本自定义信息（名称 / 描述 / 分类 / 收藏）。 */
export async function renameVersion(
  dir: string,
  versionId: string,
  patch: {
    name: string;
    description: string;
    category: string;
    favorite: boolean;
  }
): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("version_rename", { dir, versionId, ...patch });
  window.dispatchEvent(new Event("nickel:instances-changed"));
  const selected = getLaunchInstance();
  if (selected?.root === dir && selected.versionId === versionId) {
    setLaunchInstance({ ...selected, displayName: patch.name || versionId });
  }
}

/** 删除版本目录（含 jar 与版本 JSON）。二次确认由调用方负责。 */
export async function deleteVersion(
  dir: string,
  versionId: string
): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("version_delete", { dir, versionId });
  const launch = getLaunchInstance();
  if (launch?.root === dir && launch.versionId === versionId)
    setLaunchInstance(null);
  const focus = getVersionFocus();
  if (focus?.root === dir && focus.versionId === versionId)
    setVersionFocus(null);
  window.dispatchEvent(new Event("nickel:instances-changed"));
}

/** 补全文件使用统一队列；取消等待项返回 null，不产生失败提示。 */
export async function completeVersionFiles(
  dir: string,
  versionId: string
): Promise<string | null> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const settings = normalizeAppSettings(
    JSON.parse(await invoke<string>("settings_get"))
  );
  return startManagedDownload(
    {
      dir,
      versionId,
      loader: "",
      loaderVersion: null,
      source: settings.fileDownloadSource,
      kind: "version",
    },
    () => invoke<string | null>("version_complete_files", { dir, versionId })
  );
}

/** 解析实际会使用的 Java；手动路径不兼容时会抛出可执行错误。 */
export async function resolveJava(
  args: JavaResolveArgs
): Promise<JavaSelection> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  if (!args.mcVersion.trim())
    throw new Error("缺少 Minecraft 版本，无法解析 Java");
  const raw = await invoke<unknown>("java_resolve", {
    mcVersion: args.mcVersion,
    loader: args.loader,
    loaderVersion: args.loaderVersion,
    releaseTime: args.releaseTime,
    javaPath: args.javaPath,
  });
  const data = toData(raw);
  const record =
    data && typeof data === "object" ? (data as Record<string, unknown>) : {};
  const requirementRaw =
    record.requirement && typeof record.requirement === "object"
      ? (record.requirement as Record<string, unknown>)
      : {};
  const minMajor = javaNumber(requirementRaw.minMajor) ?? 8;
  const preferredMajor = javaNumber(requirementRaw.preferredMajor) ?? minMajor;
  const source: JavaSelectionSource =
    record.source === "manual" || record.source === "auto"
      ? record.source
      : "unavailable";
  return {
    path: optionalString(record.path),
    source,
    version: typeof record.version === "string" ? record.version : "",
    vendor: typeof record.vendor === "string" ? record.vendor : "",
    major: javaNumber(record.major),
    bits: javaNumber(record.bits),
    compatible: record.compatible === true,
    needsDownload: record.needsDownload === true,
    reason: typeof record.reason === "string" ? record.reason : "",
    requirement: {
      minMajor,
      maxMajor: javaNumber(requirementRaw.maxMajor),
      preferredMajor,
      reason:
        typeof requirementRaw.reason === "string" ? requirementRaw.reason : "",
    },
  };
}

/** 统一设置页展示：来源、版本、兼容性和下载状态均来自实际解析结果。 */
export function describeJavaSelection(selection: JavaSelection): string {
  const maxMajor = selection.requirement.maxMajor;
  const required =
    maxMajor === null
      ? `Java ${selection.requirement.minMajor} 以上`
      : maxMajor === selection.requirement.minMajor
      ? `Java ${maxMajor}`
      : `Java ${selection.requirement.minMajor}-${maxMajor}`;
  if (selection.needsDownload) {
    return `未找到兼容 Java，将下载 Java ${selection.requirement.preferredMajor} · 需要 ${required} · ${selection.reason}`;
  }
  if (!selection.compatible || !selection.path) {
    return `Java 不可用 · 需要 ${required} · ${selection.reason}`;
  }
  const version = selection.version || `Java ${selection.major ?? "未知"}`;
  const bits = selection.bits ? `（${selection.bits} 位）` : "";
  const source = selection.source === "manual" ? "手动选择" : "自动选择";
  return `将使用 Java ${version}${bits} · ${
    selection.vendor || "未知厂商"
  } · ${source} · 兼容`;
}

/** 探测本机可用的 Java（含内置运行时）。 */
export async function detectJava(): Promise<JavaInstallation[]> {
  if (!isTauri()) return [];
  const raw = await invoke<unknown>("java_detect");
  const data = toData(raw);
  if (!Array.isArray(data)) return [];
  const list: JavaInstallation[] = [];
  for (const item of data) {
    if (!item || typeof item !== "object") continue;
    const record = item as Record<string, unknown>;
    const path = optionalString(record.path);
    if (!path) continue;
    list.push({
      path,
      version: typeof record.version === "string" ? record.version : "",
      vendor: typeof record.vendor === "string" ? record.vendor : "",
      isDefault: record.isDefault === true,
      major: javaNumber(record.major) ?? 0,
      bits: javaNumber(record.bits) ?? 0,
      architecture:
        typeof record.architecture === "string"
          ? record.architecture
          : "unknown",
      executable: record.executable === true,
    });
  }
  return list;
}

/** 从任意错误值里取出可展示的文案。 */
export function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "操作失败，请重试。";
}
