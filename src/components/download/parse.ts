/**
 * 下载中心 · 后端返回值与事件的归一化（纯函数）。
 *
 * 后端一律以「JSON 文本」返回清单类数据，事件负载字段做宽松兜底：
 * 任何残缺数据都收敛为可用结构或 `null`，绝不抛出未捕获异常。
 */
import { toData } from "../resources/parse";
import {
  LOADER_IDS,
  type LoaderArtifactMetadata,
  type LoaderCapability,
  type LoaderProvider,
  type LoaderProviderState,
  type LoaderRawMetadataValue,
} from "./types";
import type {
  InstallEvent,
  LoaderCatalog,
  LoaderOption,
  RemoteManifest,
  RemoteVersion,
} from "./types";

/**
 * artifact.rawMetadata 的跨加载器安全子集。URL、候选地址、凭据和嵌套对象
 * 一律不进入前端；OptiFine 的 forgeRequirement 是当前共装筛选的实际消费字段。
 */
const LOADER_ARTIFACT_METADATA_KEYS = new Set([
  "minecraftId",
  "version",
  "displayName",
  "fileName",
  "fileType",
  "patch",
  "isPreview",
  "releaseTime",
  "forgeRequirement",
  "route",
  "optifineVersion",
  "branch",
  "minecraft",
  "category",
  "format",
  "apiName",
  "coordinate",
  "installerVersion",
  "optifabricMatches",
  "fabricApiMatches",
]);
const ARTIFACT_METADATA_MAX_STRING = 256;

type RawPayload = Record<string, unknown>;

/** 从负载中按候选键名取出第一个有限数字。 */
function pickNumber(payload: RawPayload, keys: string[]): number | null {
  for (const key of keys) {
    const value = payload[key];
    if (typeof value === "number" && Number.isFinite(value)) return value;
    if (typeof value === "string" && value.trim() !== "") {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) return parsed;
    }
  }
  return null;
}

/** 从负载中按候选键名取出第一个字符串（缺失时返回空串）。 */
function pickString(payload: RawPayload, keys: string[]): string {
  for (const key of keys) {
    const value = payload[key];
    if (typeof value === "string") return value.trim();
  }
  return "";
}

/** 从负载中按候选键名取出第一个布尔值。 */
function pickBoolean(
  payload: RawPayload,
  keys: string[],
  fallback: boolean
): boolean {
  for (const key of keys) {
    const value = payload[key];
    if (typeof value === "boolean") return value;
  }
  return fallback;
}

/** 收敛为 0–100 的整数百分比。 */
function clampPercent(value: number): number {
  return Math.max(0, Math.min(100, Math.round(value)));
}

function safeWebUrl(raw: unknown): string {
  if (typeof raw !== "string") return "";
  try {
    const url = new URL(raw);
    return url.protocol === "https:" && !url.username && !url.password
      ? url.href
      : "";
  } catch {
    return "";
  }
}

function capability(
  raw: unknown,
  fallback: LoaderCapability
): LoaderCapability {
  if (!raw || typeof raw !== "object") return fallback;
  const value = raw as RawPayload;
  return {
    install: value.install === true,
    saveInstaller: value.saveInstaller === true,
  };
}

function safeMetadataValue(value: unknown): LoaderRawMetadataValue | undefined {
  if (value === null || typeof value === "boolean") return value;
  if (typeof value === "number")
    return Number.isFinite(value) ? value : undefined;
  if (typeof value !== "string") return undefined;
  if (
    value.length > ARTIFACT_METADATA_MAX_STRING ||
    /[\u0000-\u001f\u007f]/.test(value) ||
    /^(?:https?:|data:|blob:|file:)/i.test(value)
  ) {
    return undefined;
  }
  return value;
}

function normalizeLoaderArtifact(
  raw: unknown
): LoaderArtifactMetadata | undefined {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return undefined;
  const entry = raw as RawPayload;
  const artifact =
    entry.artifact &&
    typeof entry.artifact === "object" &&
    !Array.isArray(entry.artifact)
      ? (entry.artifact as RawPayload)
      : null;
  const metadata =
    artifact?.rawMetadata &&
    typeof artifact.rawMetadata === "object" &&
    !Array.isArray(artifact.rawMetadata)
      ? (artifact.rawMetadata as RawPayload)
      : null;
  if (!metadata) return undefined;
  const safe: Record<string, LoaderRawMetadataValue> = {};
  for (const key of LOADER_ARTIFACT_METADATA_KEYS) {
    if (!(key in metadata)) continue;
    const value = safeMetadataValue(metadata[key]);
    if (value !== undefined) safe[key] = value;
  }
  return Object.keys(safe).length > 0 ? { rawMetadata: safe } : undefined;
}

function normalizeLoaderOptions(
  raw: unknown,
  supportsInstall = true
): LoaderOption[] {
  if (!Array.isArray(raw)) return [];
  const options: LoaderOption[] = [];
  const seen = new Set<string>();
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const entry = item as RawPayload;
    const version =
      typeof entry.version === "string" ? entry.version.trim() : "";
    if (!version || seen.has(version)) continue;
    seen.add(version);
    const downloadUrl = safeWebUrl(entry.downloadUrl);
    const artifact = normalizeLoaderArtifact(entry);
    options.push({
      version,
      label: pickString(entry, ["label"]) || version,
      releaseTime: pickString(entry, ["releaseTime"]),
      modifiedTime: pickString(entry, ["modifiedTime"]),
      stable: typeof entry.stable === "boolean" ? entry.stable : null,
      latest: entry.latest === true,
      recommended: entry.recommended === true,
      downloadUrl,
      changelogUrl: safeWebUrl(entry.changelogUrl),
      capability: capability(entry.capability, {
        install: supportsInstall,
        saveInstaller: !!downloadUrl,
      }),
      ...(artifact ? { artifact } : {}),
    });
  }
  return options;
}

/** 归一化 `list_remote_versions`：结构不符时返回空清单（界面据此给出可执行提示）。 */
export function normalizeManifest(raw: unknown): RemoteManifest {
  const empty: RemoteManifest = {
    latestRelease: "",
    latestSnapshot: "",
    versions: [],
  };
  const data = toData(raw);
  if (!data || typeof data !== "object") return empty;
  const record = data as RawPayload;

  const latest =
    record.latest && typeof record.latest === "object"
      ? (record.latest as RawPayload)
      : ({} as RawPayload);

  const versions: RemoteVersion[] = [];
  const rawVersions = Array.isArray(record.versions) ? record.versions : [];
  for (const item of rawVersions) {
    if (!item || typeof item !== "object") continue;
    const entry = item as RawPayload;
    const id = typeof entry.id === "string" ? entry.id.trim() : "";
    if (!id) continue;
    versions.push({
      id,
      type:
        typeof entry.type === "string" && entry.type.trim()
          ? entry.type.trim()
          : "unknown",
      releaseTime:
        typeof entry.releaseTime === "string" ? entry.releaseTime.trim() : "",
    });
  }

  const latestRelease =
    typeof latest.release === "string" ? latest.release.trim() : "";
  const latestSnapshot =
    typeof latest.snapshot === "string" ? latest.snapshot.trim() : "";
  if (versions.length === 0 && !latestRelease && !latestSnapshot) return empty;

  return { latestRelease, latestSnapshot, versions };
}

/** 归一化 `list_loaders`：缺失的加载器一律视为「无可用版本」。 */
export function normalizeLoaders(raw: unknown): LoaderCatalog {
  const data = toData(raw);
  if (!data || typeof data !== "object" || Array.isArray(data))
    throw new Error("加载器清单格式无法识别，请重试。");
  const record = data as RawPayload;
  if (
    !["forge", "neoforge", "fabric", "liteloader", "optifine"].some((key) =>
      Array.isArray(record[key])
    )
  )
    throw new Error("加载器清单缺少版本列表，请重试。");
  const emptyProvider = (): LoaderProvider => ({
    state: "unavailable",
    capability: { install: false, saveInstaller: false },
    error: "",
  });
  const catalog: LoaderCatalog = {
    forge: normalizeLoaderOptions(record.forge),
    neoforge: normalizeLoaderOptions(record.neoforge),
    fabric: normalizeLoaderOptions(record.fabric),
    liteloader: normalizeLoaderOptions(record.liteloader, false),
    optifine: normalizeLoaderOptions(record.optifine, false),
    notes: {},
    providers: {
      forge: emptyProvider(),
      neoforge: emptyProvider(),
      fabric: emptyProvider(),
      liteloader: emptyProvider(),
      optifine: emptyProvider(),
    },
  };
  const rawNotes = record.notes;
  if (rawNotes && typeof rawNotes === "object")
    for (const [key, value] of Object.entries(rawNotes)) {
      if (typeof value === "string" && value.trim())
        catalog.notes[key] = value.trim();
    }
  const providers =
    record.providers && typeof record.providers === "object"
      ? (record.providers as RawPayload)
      : {};
  const states: LoaderProviderState[] = [
    "hidden",
    "loading",
    "available",
    "unavailable",
    "failed",
    "manualOnly",
    "incompatible",
  ];
  for (const id of LOADER_IDS) {
    const rawProvider = providers[id];
    const provider =
      rawProvider && typeof rawProvider === "object"
        ? (rawProvider as RawPayload)
        : {};
    const options = catalog[id];
    const fallback = {
      install: options.some((option) => option.capability.install),
      saveInstaller: options.some((option) => option.capability.saveInstaller),
    };
    const providerCapability = capability(provider.capability, fallback);
    catalog.providers[id] = {
      state: states.includes(provider.state as LoaderProviderState)
        ? (provider.state as LoaderProviderState)
        : providerCapability.install
        ? "available"
        : providerCapability.saveInstaller
        ? "manualOnly"
        : "unavailable",
      capability: providerCapability,
      error: pickString(provider, ["error"]),
    };
  }
  return catalog;
}

/**
 * 归一化 `install-event`。
 * 未知事件类型 / 无法识别负载返回 null，由调用方静默忽略（绝不崩溃）。
 */
export function parseInstallEvent(raw: unknown): InstallEvent | null {
  if (!raw || typeof raw !== "object") return null;
  const payload = raw as RawPayload;
  const kind = pickString(payload, ["type"]);
  const taskId = pickString(payload, ["taskId"]);
  if (!taskId) return null;
  switch (kind) {
    case "stage":
      return {
        kind: "stage",
        taskId,
        stage: pickString(payload, ["stage"]),
        detail: pickString(payload, ["detail"]),
      };
    case "progress": {
      const percent = pickNumber(payload, ["percent"]);
      return {
        kind: "progress",
        taskId,
        percent: percent === null ? null : clampPercent(percent * 100),
        filesDone: Math.max(0, pickNumber(payload, ["filesDone"]) ?? 0),
        filesTotal: Math.max(0, pickNumber(payload, ["filesTotal"]) ?? 0),
        speedBps: pickNumber(payload, ["speedBps"]),
        remainingMs: pickNumber(payload, ["remainingMs"]),
      };
    }
    case "file": {
      const name = pickString(payload, ["name"]);
      if (!name) return null;
      return {
        kind: "file",
        taskId,
        name,
        percent: clampPercent((pickNumber(payload, ["percent"]) ?? 0) * 100),
        done: pickBoolean(payload, ["done"], false),
      };
    }
    case "done": {
      const rawResult = payload.result;
      const result =
        rawResult && typeof rawResult === "object"
          ? (rawResult as RawPayload)
          : {};
      const root = pickString(result, ["root"]);
      const versionId = pickString(result, ["versionId"]);
      return {
        kind: "done",
        taskId,
        ok: pickBoolean(payload, ["ok"], false),
        message: pickString(payload, ["message"]),
        cancelled: pickBoolean(payload, ["cancelled"], false),
        ...(root && versionId
          ? {
              result: {
                root,
                versionId,
                displayName: pickString(result, ["displayName"]) || versionId,
                source: result.source,
              },
            }
          : {}),
      };
    }
    case "error":
      return {
        kind: "error",
        taskId,
        message:
          pickString(payload, ["message"]) || "下载失败",
      };
    default:
      return null;
  }
}

/** 把安装失败原因翻译成可执行的中文提示。 */
export function describeInstallError(err: unknown): string {
  const message =
    err instanceof Error ? err.message : typeof err === "string" ? err : "";
  if (/not found|unknown command|not_allowed|no such/i.test(message)) {
    return "当前构建未提供版本安装命令，无法开始下载。";
  }
  if (/permission|denied|forbidden|access is denied/i.test(message)) {
    return "系统拒绝写入实例目录，请检查目录权限后重试。";
  }
  if (
    /timeout|timed out|network|connect|dns|resolve|dns error/i.test(message)
  ) {
    return "网络请求失败，请检查网络连接或切换下载源后重试。";
  }
  return message || "下载失败，请重试。";
}

/** 从实例 id 中提取 MC 版本号（`1.20.1-forge-47.2.0` → `1.20.1`）。 */
export function extractMcVersion(id: string): string {
  const match = id.match(/^(\d+\.\d+(?:\.\d+)?)/);
  return match ? match[1] : id;
}
