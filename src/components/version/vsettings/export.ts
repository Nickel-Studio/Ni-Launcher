import { invoke } from "@tauri-apps/api/core";
import { PRODUCT_NAME } from "../../../lib/brand";
import { isTauri, type SourcePolicy } from "../../../lib/settings";
import { startManagedDownload } from "../../download/store";

export const EXPORT_GROUPS = [
  { id: "gameOptions", label: "游戏本体设置" },
  { id: "personal", label: "游戏本体个人信息" },
  { id: "mods", label: "Mod" },
  { id: "config", label: "Mod 设置" },
  { id: "packData", label: "整合包重要数据" },
  { id: "jeiPersonal", label: "JEI 个人数据" },
  { id: "emiPersonal", label: "EMI 个人数据" },
  { id: "patchouliPersonal", label: "帕秋莉个人数据" },
  { id: "maps", label: "已绘制的个人地图" },
  { id: "tacz", label: "TaCZ 枪包" },
  { id: "paintings", label: "已上传的沉浸画" },
  { id: "resourcepacks", label: "资源包" },
  { id: "shaderpacks", label: "光影包" },
  { id: "shaderOptions", label: "光影包设置" },
  { id: "schematics", label: "导出的结构" },
  { id: "replays", label: "录像回放与视频" },
  { id: "screenshots", label: "截图" },
  { id: "saves", label: "单人游戏存档" },
  { id: "servers", label: "多人游戏服务器列表" },
] as const;

export type ExportGroup = (typeof EXPORT_GROUPS)[number]["id"] | "disabledMods";

function isExportGroup(value: unknown): value is ExportGroup {
  return (
    value === "disabledMods" ||
    EXPORT_GROUPS.some((group) => group.id === value)
  );
}

export function exportDisplayGroup(
  group: ExportGroup
): (typeof EXPORT_GROUPS)[number]["id"] {
  return group === "disabledMods" ? "mods" : group;
}
export type ExportFormat = "mrpack" | "portable";
export interface ExportFile {
  // New previews contain selection tokens, not recursively expanded file lists.
  path: string;
  sourcePath: string;
  displayName: string;
  kind: "file" | "directory";
  sizeBytes: number | null;
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

export const BUNDLE_RESOURCES_NOTICE =
  "建议仅在无法稳定连接CurseForge或Modrinth 时才打包资源文件。\n公开发布含资源文件的整合包可能会违反部分Mod的使用协议。";

function safeRelativePath(path: string): boolean {
  if (
    !path ||
    path.length > 1024 ||
    new TextEncoder().encode(path).length > 1024 ||
    /[\\:<>"|?*\u0000-\u001f\u007f-\u009f]/.test(path)
  )
    return false;
  return path.split("/").every((part) => {
    const stem = part.split(".")[0].toUpperCase();
    return (
      part !== "" &&
      part !== "." &&
      part !== ".." &&
      !/[. ]$/.test(part) &&
      !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/.test(stem)
    );
  });
}

function within(path: string, directory: string): boolean {
  return path === directory || path.startsWith(`${directory}/`);
}

function fileGroup(path: string): ExportGroup | null {
  if (!safeRelativePath(path)) return null;
  const lower = path.toLowerCase();
  const world = /^saves\/[^/]+(?:\/|$)/.test(lower);
  const atomic = /^(saves|resourcepacks|shaderpacks)\/[^/]+(?:\/|$)/.test(
    lower
  );
  if (
    lower.split("/").some((part, index) => {
      if (world && index === 1) return false;
      const stem = part.replace(/\.[^.]*$/, "").replace(/[_ -]/g, "");
      return (
        part.startsWith(".") ||
        part.startsWith("nickel.") ||
        /^(secrets?|credentials?|passwords?|accesstoken|clienttoken|refreshtoken|authtoken|bearertoken|sessiontoken|apitoken|clientsecret|apikeys?|authorization|privatekey|privatetoken|secretkey)$/.test(
          stem
        ) ||
        /\.(pem|key|p12)$/.test(part) ||
        [
          "accounts.json",
          "launcher_accounts.json",
          "launcher_profiles.json",
          "usercache.json",
          "usernamecache.json",
        ].includes(part) ||
        (!atomic &&
          [
            "cache",
            "caches",
            "cached",
            "probe",
            "exported",
            "playerdata",
            "stats",
          ].includes(part))
      );
    })
  )
    return null;
  if (within(lower, "config/jei/world")) return "jeiPersonal";
  if (["emi.json", "config/emi.json"].includes(lower)) return "emiPersonal";
  if (["patchouli_data.json", "config/patchouli_data.json"].includes(lower))
    return "patchouliPersonal";
  if (within(lower, "config/tacz/custom")) return "tacz";
  if (
    [
      "config/openloader",
      "config/global_packs",
      "config/paxi/datapacks",
      "config/paxi/resourcepacks",
      "config/packmenu",
      "config/patchouli_books",
    ].some((scope) => within(lower, scope))
  )
    return "packData";
  if (
    [
      "optionsshaders.txt",
      "config/iris.properties",
      "config/oculus.properties",
    ].includes(lower) ||
    /^shaderpacks\/[^/]+\.(txt|properties|cfg|ini|json|toml|yaml|yml)$/.test(
      lower
    )
  )
    return "shaderOptions";
  if (
    within(lower, "journeymap/config") ||
    within(lower, "journeymap/server") ||
    /^journeymap\/[^/]+\.(json|cfg|conf|config|properties|toml)$/.test(lower)
  )
    return "config";
  const root = lower.split("/")[0];
  switch (root) {
    case "mods":
      return /\.(disabled|disable|old)$/.test(lower) ? "disabledMods" : "mods";
    case "config":
    case "defaultconfigs":
      return "config";
    case "kubejs":
    case "scripts":
    case "patchouli_books":
    case "datapacks":
    case "openloader":
    case "global_packs":
    case "resources":
    case "structures":
    case "packmenu":
    case "paxi":
      return "packData";
    case "tacz":
      return "tacz";
    case "immersive_paintings":
      return "paintings";
    case "schematics":
      return "schematics";
    case "replay_recordings":
    case "replay_videos":
      return "replays";
    case "resourcepacks":
    case "shaderpacks":
    case "screenshots":
    case "saves":
      return root;
    case "xaero":
    case "xaerowaypoints":
    case "xaeroworldmap":
    case "journeymap":
    case "voxelmap":
      return "maps";
    case "options.txt":
    case "optionsof.txt":
      return lower === root ? "gameOptions" : null;
    case "servers.dat":
    case "servers.dat_old":
      return lower === root ? "servers" : null;
    case "hotbar.nbt":
    case "command_history.txt":
      return lower === root ? "personal" : null;
    default:
      return null;
  }
}

function validSelection(value: string): boolean {
  if (!value.startsWith("@dir/")) return fileGroup(value) !== null;
  const rest = value.slice(5);
  const separator = rest.indexOf("/");
  if (separator < 1) return false;
  const group = rest.slice(0, separator);
  const relative = rest.slice(separator + 1);
  if (!safeRelativePath(relative) || !isExportGroup(group)) return false;
  if (relative === "journeymap") return group === "config" || group === "maps";
  if (relative === "shaderpacks") return group === "shaderOptions";
  if (relative.startsWith("mods/"))
    return (
      (group === "mods" || group === "disabledMods") &&
      fileGroup(relative) !== null
    );
  const nestedScopes = [
    "config/jei/world",
    "config/tacz/custom",
    "config/openloader",
    "config/global_packs",
    "config/paxi/datapacks",
    "config/paxi/resourcepacks",
    "config/packmenu",
    "config/patchouli_books",
  ];
  const atomicPackage = /^(resourcepacks|shaderpacks|saves)\/[^/]+$/.test(
    relative
  );
  const rootDirectory =
    !relative.includes("/") &&
    !["mods", "resourcepacks", "shaderpacks", "saves"].includes(relative) &&
    !relative.includes(".");
  return (
    (nestedScopes.includes(relative) || atomicPackage || rootDirectory) &&
    fileGroup(relative) === group
  );
}

export type ExportSelection = ReadonlyMap<string, readonly string[]>;
export type ExportCheckState = "checked" | "mixed" | "unchecked";

export function exportCheckState(
  file: ExportFile,
  selection: ExportSelection
): ExportCheckState {
  const paths = selection.get(file.path);
  if (!paths?.length) return "unchecked";
  return paths.length === 1 && paths[0] === file.path ? "checked" : "mixed";
}

export function defaultExportSelection(
  preview: ExportPreview
): Map<string, readonly string[]> {
  return new Map(
    preview.files
      .filter((file) => file.selected)
      .map((file) => [file.path, [file.path]])
  );
}

export function matchExportSelection(
  preview: ExportPreview,
  paths: readonly string[]
): {
  selection: Map<string, readonly string[]>;
  missing: number;
  legacy: number;
} {
  const exact = new Map(preview.files.map((file) => [file.path, file]));
  const directories = new Map(
    preview.files
      .filter((file) => file.kind === "directory")
      .map((file) => [`${file.group}/${file.sourcePath}`, file])
  );
  const buckets = new Map<string, Set<string>>();
  let missing = 0;
  let legacy = 0;
  for (const path of paths) {
    if (!validSelection(path)) throw new Error("导出配置含不允许的选择路径");
    let entry = exact.get(path);
    if (!entry && !path.startsWith("@dir/")) {
      const group = fileGroup(path);
      let ancestor = path;
      while (!entry && ancestor.includes("/")) {
        ancestor = ancestor.slice(0, ancestor.lastIndexOf("/"));
        entry = directories.get(`${group}/${ancestor}`);
      }
    }
    if (!entry) {
      missing += 1;
      continue;
    }
    const bucket = buckets.get(entry.path) ?? new Set<string>();
    if (!bucket.has(entry.path)) {
      if (path === entry.path) bucket.clear();
      bucket.add(path);
    }
    buckets.set(entry.path, bucket);
    if (path !== entry.path) legacy += 1;
  }
  return {
    selection: new Map([...buckets].map(([key, values]) => [key, [...values]])),
    missing,
    legacy,
  };
}

export function toggleExportSelection(
  previous: ExportSelection,
  entries: readonly string[],
  checked: boolean
): Map<string, readonly string[]> {
  const next = new Map(previous);
  for (const entry of entries) {
    if (checked) next.set(entry, [entry]);
    else next.delete(entry);
  }
  return next;
}

export function serializeExportSelection(selection: ExportSelection): string[] {
  return [...new Set([...selection.values()].flat())];
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
  const seen = new Set<string>();
  const files = value.files.map((item: unknown): ExportFile => {
    const file = asRecord(item);
    const kind = file.kind === undefined ? "file" : file.kind;
    const sourcePath =
      file.sourcePath === undefined ? file.path : file.sourcePath;
    const displayName =
      file.displayName === undefined && typeof sourcePath === "string"
        ? sourcePath.split("/").pop()
        : file.displayName;
    const sizeBytes = file.sizeBytes;
    if (
      typeof file.path !== "string" ||
      !file.path ||
      typeof sourcePath !== "string" ||
      !sourcePath ||
      typeof displayName !== "string" ||
      !displayName.trim() ||
      (kind !== "file" && kind !== "directory") ||
      (sizeBytes !== null &&
        (typeof sizeBytes !== "number" ||
          !Number.isFinite(sizeBytes) ||
          sizeBytes < 0)) ||
      (kind === "file" && sizeBytes === null) ||
      !isExportGroup(file.group) ||
      typeof file.selected !== "boolean" ||
      seen.has(file.path.toLowerCase())
    ) {
      throw new Error("导出文件信息不完整，请重新读取");
    }
    if (
      (kind === "directory" &&
        file.path !== `@dir/${file.group}/${sourcePath}`) ||
      (kind === "file" && file.path !== sourcePath) ||
      !safeRelativePath(sourcePath)
    ) {
      throw new Error("导出选择路径无效，请重新读取");
    }
    seen.add(file.path.toLowerCase());
    return {
      path: file.path,
      sourcePath,
      displayName,
      kind,
      sizeBytes,
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
    config.files.length > 100_000 ||
    !config.files.every((path: unknown) => typeof path === "string") ||
    typeof bundleResources !== "boolean" ||
    typeof modrinthOnly !== "boolean" ||
    (format !== "mrpack" && format !== "portable") ||
    typeof includeLauncher !== "boolean" ||
    typeof includePersonalization !== "boolean"
  ) {
    throw new Error("导出配置格式无法识别，请选择有效的配置文件");
  }
  const seen = new Set<string>();
  for (const path of config.files as string[]) {
    if (!validSelection(path) || seen.has(path.toLowerCase()))
      throw new Error("导出配置含不允许或重复的选择路径");
    seen.add(path.toLowerCase());
  }
  // Explicit fields keep request identity and preview metadata out of saved configs.
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
