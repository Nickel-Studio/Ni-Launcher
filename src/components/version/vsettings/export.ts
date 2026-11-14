import { invoke } from "@tauri-apps/api/core";
import { PRODUCT_NAME } from "../../../lib/brand";
import { isTauri, type SourcePolicy } from "../../../lib/settings";
import { startManagedDownload } from "../../download/store";

export const EXPORT_GROUPS = [
  { id: "gameOptions", label: "游戏本体设置" },
  { id: "personal", label: "游戏本体个人信息" },
  { id: "mods", label: "Mod" },
  { id: "disabledMods", label: "已禁用的 Mod" },
  { id: "config", label: "Mod 设置" },
  { id: "maps", label: "已绘制的地图" },
  { id: "resourcepacks", label: "资源包" },
  { id: "shaderpacks", label: "光影包" },
  { id: "shaderOptions", label: "光影包设置" },
  { id: "screenshots", label: "截图" },
  { id: "saves", label: "单人游戏存档" },
  { id: "servers", label: "多人游戏服务器列表" },
] as const;

export type ExportGroup = (typeof EXPORT_GROUPS)[number]["id"];
export type ExportFormat = "mrpack" | "portable";
export interface ExportFile {
  path: string;
  sizeBytes: number;
  group: ExportGroup;
  selected: boolean;
}
export interface LauncherAvailability {
  available: boolean;
  detail: string;
  os: string;
  arch: string;
}
export interface ExportPreview {
  name: string;
  minecraftVersion: string;
  loader: { kind: string; version: string } | null;
  files: ExportFile[];
  launcher: LauncherAvailability;
  instanceSettings: string[];
  instanceSettingsOmitted: string[];
  instanceSettingsError: string | null;
}
export interface ExportConfig {
  name: string;
  version: string;
  files: string[];
  bundleResources: boolean;
  modrinthOnly: boolean;
  format: ExportFormat;
  includeLauncher: boolean;
  includePersonalization: boolean;
}
export interface PackExportRequest extends ExportConfig {
  dir: string;
  versionId: string;
}

export const DEFAULT_PACK_VERSION = "1.0.0";
export const EXPORT_FORMATS: readonly {
  value: ExportFormat;
  label: string;
  extension: string;
}[] = [
  { value: "mrpack", label: "标准整合包", extension: ".mrpack" },
  { value: "portable", label: "便携 ZIP", extension: ".zip" },
];

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("导出数据格式无法识别，请重新读取");
  return value as Record<string, unknown>;
}

function stringList(value: unknown, label: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || !value.every((item) => typeof item === "string"))
    throw new Error(`${label}格式无法识别，请重新读取`);
  return [...value] as string[];
}

export function normalizeExportPreview(raw: unknown): ExportPreview {
  const value = asRecord(raw);
  if (
    typeof value.name !== "string" ||
    typeof value.minecraftVersion !== "string" ||
    !Array.isArray(value.files)
  ) {
    throw new Error("导出内容信息不完整，请重新读取");
  }
  const loader = value.loader == null ? null : asRecord(value.loader);
  if (
    loader &&
    (typeof loader.kind !== "string" || typeof loader.version !== "string")
  )
    throw new Error("整合包加载器信息不完整，请重新读取");
  const files = value.files.map((item: unknown): ExportFile => {
    const file = asRecord(item);
    if (
      typeof file.path !== "string" ||
      !file.path ||
      typeof file.sizeBytes !== "number" ||
      !Number.isFinite(file.sizeBytes) ||
      file.sizeBytes < 0 ||
      !EXPORT_GROUPS.some((group) => group.id === file.group) ||
      typeof file.selected !== "boolean"
    ) {
      throw new Error("导出文件信息不完整，请重新读取");
    }
    return {
      path: file.path,
      sizeBytes: file.sizeBytes,
      group: file.group as ExportGroup,
      selected: file.selected,
    };
  });
  const launcher =
    value.launcher === undefined
      ? {
          available: false,
          detail: `请更新 ${PRODUCT_NAME} 后重新读取启动器打包信息`,
          os: "",
          arch: "",
        }
      : asRecord(value.launcher);
  if (
    typeof launcher.available !== "boolean" ||
    typeof launcher.detail !== "string" ||
    typeof launcher.os !== "string" ||
    typeof launcher.arch !== "string"
  ) {
    throw new Error("启动器打包信息不完整，请重新读取");
  }
  const instanceSettings = stringList(value.instanceSettings, "便携实例设置");
  const instanceSettingsOmitted = stringList(
    value.instanceSettingsOmitted,
    "未迁移实例设置"
  );
  const instanceSettingsError =
    value.instanceSettingsError === undefined ||
    value.instanceSettingsError === null
      ? null
      : typeof value.instanceSettingsError === "string"
      ? value.instanceSettingsError
      : (() => {
          throw new Error("实例设置错误信息格式无法识别，请重新读取");
        })();
  return {
    name: value.name,
    minecraftVersion: value.minecraftVersion,
    loader: loader
      ? { kind: loader.kind as string, version: loader.version as string }
      : null,
    files,
    launcher: {
      available: launcher.available,
      detail: launcher.detail,
      os: launcher.os,
      arch: launcher.arch,
    },
    instanceSettings,
    instanceSettingsOmitted,
    instanceSettingsError,
  };
}

export function normalizeExportConfig(raw: unknown): ExportConfig {
  const config = asRecord(raw);
  const format = config.format === undefined ? "mrpack" : config.format;
  const includeLauncher =
    config.includeLauncher === undefined ? false : config.includeLauncher;
  const includePersonalization =
    config.includePersonalization === undefined
      ? false
      : config.includePersonalization;
  const bundleResources =
    config.bundleResources === undefined ? false : config.bundleResources;
  const modrinthOnly =
    config.modrinthOnly === undefined ? false : config.modrinthOnly;
  if (
    typeof config.name !== "string" ||
    typeof config.version !== "string" ||
    !Array.isArray(config.files) ||
    !config.files.every((path: unknown) => typeof path === "string") ||
    typeof bundleResources !== "boolean" ||
    typeof modrinthOnly !== "boolean" ||
    (format !== "mrpack" && format !== "portable") ||
    typeof includeLauncher !== "boolean" ||
    typeof includePersonalization !== "boolean"
  ) {
    throw new Error("导出配置格式无法识别，请选择有效的配置文件");
  }
  // 显式挑选字段，保证保存配置不会夹带请求身份；标准格式没有便携附加内容。
  return {
    name: config.name,
    version: config.version,
    files: [...config.files] as string[],
    bundleResources,
    modrinthOnly,
    format,
    includeLauncher: format === "portable" && includeLauncher,
    includePersonalization: format === "portable" && includePersonalization,
  };
}

export async function inspectPackExport(
  dir: string,
  versionId: string
): Promise<ExportPreview> {
  if (!isTauri()) throw new Error("请在启动器中读取导出内容");
  return normalizeExportPreview(
    await invoke<unknown>("pack_export_inspect", { dir, versionId })
  );
}

export async function loadExportConfig(): Promise<ExportConfig | null> {
  if (!isTauri()) throw new Error("请在启动器中读取导出配置");
  const config = await invoke<unknown>("pack_export_config_load", {});
  return config === null ? null : normalizeExportConfig(config);
}

export async function saveExportConfig(
  config: ExportConfig
): Promise<string | null> {
  if (!isTauri()) throw new Error("请在启动器中保存导出配置");
  // 后端 deny_unknown_fields；不能将 dir / versionId 或预览字段带进配置。
  return invoke<string | null>("pack_export_config_save", {
    config: normalizeExportConfig(config),
  });
}

export async function startPackExport(
  request: PackExportRequest,
  source: SourcePolicy
): Promise<string | null> {
  if (!isTauri()) throw new Error("请在启动器中导出整合包");
  return startManagedDownload(
    {
      dir: request.dir,
      versionId: request.versionId,
      instanceName: request.name,
      loader: "",
      loaderVersion: null,
      source,
      kind: "export",
    },
    () =>
      invoke<string | null>("pack_export_save", {
        request: {
          dir: request.dir,
          versionId: request.versionId,
          ...normalizeExportConfig(request),
        },
      })
  );
}
