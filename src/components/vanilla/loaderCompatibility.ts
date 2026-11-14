/** 主加载器互斥；安装链只接受一个加载器，OptiFine 组合须另存后手动安装。 */
import {
  LOADER_LABELS,
  type LoaderId,
  type LoaderOption,
  type LoaderProviderState,
} from "../download/types";

export type MainLoaderId = Exclude<LoaderId, "optifine">;

export const MAIN_LOADER_IDS: readonly MainLoaderId[] = [
  "forge",
  "neoforge",
  "fabric",
  "liteloader",
];

export type LoaderSelection = {
  id: LoaderId;
  version: string;
};

export type OptiFineCompatibility = "available" | "manualOnly" | "incompatible";

export interface OptiFineCompatibilityResult {
  state: OptiFineCompatibility;
  reason: string;
  directory: string;
}

export interface OptiFineMetadata {
  minecraftId?: string;
  forgeRequirement?: string | null;
  optifabricMatches?: boolean;
  fabricApiMatches?: boolean;
}

export type LoaderOptionView = LoaderOption & {
  manualReason?: string;
  compatibility?: OptiFineCompatibility;
};

export interface OptiFineOptionsView {
  options: LoaderOptionView[];
  state: LoaderProviderState;
  reason: string;
}

export interface DownloadIdentity {
  loader: string;
  loaderVersion: string | null;
}

export function isMainLoaderId(id: LoaderId): id is MainLoaderId {
  return id !== "optifine";
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function firstDefined<T>(...values: (T | undefined)[]): T | undefined {
  return values.find((value) => value !== undefined);
}

function metadataString(
  raw: Record<string, unknown>,
  key: string
): string | null | undefined {
  if (!(key in raw)) return undefined;
  const value = raw[key];
  if (value === null) return null;
  return typeof value === "string" ? value.trim() : undefined;
}

function metadataBoolean(
  raw: Record<string, unknown>,
  key: string
): boolean | undefined {
  const value = raw[key];
  return typeof value === "boolean" ? value : undefined;
}

export function optifineMetadata(option: LoaderOption): OptiFineMetadata {
  const direct = option as unknown as Record<string, unknown>;
  const artifact = objectValue(direct.artifact);
  const rawMetadata = objectValue(artifact?.rawMetadata);
  const raw = rawMetadata ?? direct;
  return {
    minecraftId:
      firstDefined(
        metadataString(raw, "minecraftId"),
        metadataString(direct, "minecraftId")
      ) ?? undefined,
    forgeRequirement: firstDefined(
      metadataString(raw, "forgeRequirement"),
      metadataString(direct, "forgeRequirement")
    ),
    optifabricMatches: firstDefined(
      metadataBoolean(raw, "optifabricMatches"),
      metadataBoolean(direct, "optifabricMatches")
    ),
    fabricApiMatches: firstDefined(
      metadataBoolean(raw, "fabricApiMatches"),
      metadataBoolean(direct, "fabricApiMatches")
    ),
  };
}

export function canonicalMinecraftId(value: string): string {
  const trimmed = value.trim();
  if (trimmed !== "1.0" && trimmed.endsWith(".0")) {
    return trimmed.slice(0, -2);
  }
  return trimmed;
}

function labelMinecraftTokens(option: LoaderOption): string[] {
  const tokens = option.label.trim().split(/\s+/).filter(Boolean);
  if (tokens.length > 0) tokens.push(tokens[0]);
  for (const token of option.version.split(/[_-]/)) {
    if (/^\d+\.\d+(?:\.\d+)?$/.test(token)) tokens.push(token);
  }
  return tokens;
}

export function optifineMatchesMinecraft(
  option: LoaderOption,
  minecraftId: string
): boolean {
  const metadata = optifineMetadata(option);
  if (metadata.minecraftId) {
    return (
      canonicalMinecraftId(metadata.minecraftId) ===
      canonicalMinecraftId(minecraftId)
    );
  }
  const expected = canonicalMinecraftId(minecraftId);
  return labelMinecraftTokens(option).some(
    (token) => canonicalMinecraftId(token) === expected
  );
}

function numericParts(value: string): number[] | null {
  const match = /^(\d+(?:\.\d+)*)/.exec(value.trim());
  if (!match) return null;
  const parts = match[1].split(".").map(Number);
  return parts.every(Number.isFinite) ? parts : null;
}

function compareNumeric(left: string, right: string): number | null {
  const a = numericParts(left);
  const b = numericParts(right);
  if (!a || !b) return null;
  const length = Math.max(a.length, b.length);
  for (let index = 0; index < length; index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

export function compareMinecraftVersion(
  left: string,
  right: string
): number | null {
  return compareNumeric(left, right);
}

function inVersionRange(
  minecraftId: string,
  start: string,
  end: string
): boolean {
  const lower = compareMinecraftVersion(minecraftId, start);
  const upper = compareMinecraftVersion(minecraftId, end);
  return lower !== null && upper !== null && lower >= 0 && upper <= 0;
}

function atLeastVersion(minecraftId: string, minimum: string): boolean {
  const compared = compareMinecraftVersion(minecraftId, minimum);
  return compared !== null && compared >= 0;
}

function cleanForgeRequirement(value: string): string {
  return value
    .trim()
    .replace(/^Forge\s*/i, "")
    .replace(/^#+/, "")
    .trim();
}

function normalizedForgeBuild(build: string, minecraftId: string): string {
  return cleanForgeRequirement(build)
    .replace(new RegExp(`^${escapeRegExp(minecraftId)}-`), "")
    .replace(new RegExp(`-${escapeRegExp(minecraftId)}$`), "")
    .trim();
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * `forgeRequirement` 为 null 时不兼容；空串不限；带点版本按数值相等；
 * 纯 build 编号按 Forge 的最后一段 Revision 相等（字段名虽称“最低”）。
 */
export function forgeRequirementMatches(
  requirement: string | null | undefined,
  forgeBuild: string,
  minecraftId: string
): boolean {
  if (requirement === null || requirement === undefined) return false;
  const cleaned = cleanForgeRequirement(requirement);
  if (!cleaned) return true;
  const build = normalizedForgeBuild(forgeBuild, minecraftId);
  if (cleaned.includes(".")) {
    const compared = compareNumeric(build, cleaned);
    return compared === null ? build === cleaned : compared === 0;
  }
  const revision = build.split(".").pop() ?? "";
  return revision === cleaned;
}

export function optifineCompatibility(
  option: LoaderOption,
  minecraftId: string,
  primary: LoaderSelection | null
): OptiFineCompatibilityResult {
  if (!optifineMatchesMinecraft(option, minecraftId)) {
    return {
      state: "incompatible",
      reason: "与当前 Minecraft 版本不匹配",
      directory: "",
    };
  }

  const main = primary && isMainLoaderId(primary.id) ? primary : null;
  if (!main) {
    if (option.capability.install) {
      return { state: "available", reason: "", directory: "" };
    }
    return option.capability.saveInstaller
      ? { state: "manualOnly", reason: "仅可另存安装器", directory: "" }
      : {
          state: "incompatible",
          reason: "当前构建没有可用的安装或另存来源",
          directory: "",
        };
  }

  const metadata = optifineMetadata(option);

  if (main.id === "neoforge") {
    return {
      state: "incompatible",
      reason: "NeoForge 与 OptiFine 不兼容",
      directory: "",
    };
  }

  if (main.id === "forge") {
    if (inVersionRange(minecraftId, "1.13", "1.14.3")) {
      return {
        state: "incompatible",
        reason: "Minecraft 1.13 至 1.14.3 的 Forge 与 OptiFine 不兼容",
        directory: "",
      };
    }
    if (metadata.forgeRequirement === undefined) {
      return {
        state: "manualOnly",
        reason: "需核对 Forge 构建后另存，并手动放入 mods",
        directory: "mods",
      };
    }
    if (metadata.forgeRequirement === null) {
      return {
        state: "incompatible",
        reason: "该 OptiFine 版本未标注兼容 Forge 构建",
        directory: "",
      };
    }
    if (
      !forgeRequirementMatches(
        metadata.forgeRequirement,
        main.version,
        minecraftId
      )
    ) {
      const requirement = cleanForgeRequirement(metadata.forgeRequirement);
      return {
        state: "incompatible",
        reason: requirement
          ? `该 OptiFine 版本要求 Forge ${requirement}`
          : "OptiFine 版本要求不同的 Forge 构建",
        directory: "",
      };
    }
    return {
      state: "manualOnly",
      reason: "可另存后手动放入 mods",
      directory: "mods",
    };
  }

  if (main.id === "fabric") {
    if (atLeastVersion(minecraftId, "1.20.5")) {
      return {
        state: "incompatible",
        reason: "Minecraft 1.20.5 及以后不再支持 Fabric + OptiFine",
        directory: "",
      };
    }
    if (inVersionRange(minecraftId, "1.14", "1.15.999")) {
      return {
        state: "manualOnly",
        reason: "另存后手动安装 OptiFabric Origins，并另行准备 Fabric API",
        directory: "mods",
      };
    }
    if (metadata.optifabricMatches === false) {
      return {
        state: "incompatible",
        reason: "没有匹配当前 Minecraft 版本的 OptiFabric",
        directory: "",
      };
    }
    return {
      state: "manualOnly",
      reason:
        metadata.fabricApiMatches === false
          ? "另存后需准备匹配版本的 OptiFabric，并另行准备 Fabric API"
          : "另存后需匹配的 OptiFabric；Fabric API 是另一项依赖",
      directory: "mods",
    };
  }

  return {
    state: "manualOnly",
    reason: `另存后手动放入 mods/${minecraftId}`,
    directory: `mods/${minecraftId}`,
  };
}

export function optifineOptionsView(
  options: readonly LoaderOption[],
  minecraftId: string,
  primary: LoaderSelection | null
): OptiFineOptionsView {
  const matching = options.filter((option) =>
    optifineMatchesMinecraft(option, minecraftId)
  );
  if (matching.length === 0) {
    return { options: [], state: "unavailable", reason: "" };
  }

  const evaluated = matching.map((option) => ({
    option,
    compatibility: optifineCompatibility(option, minecraftId, primary),
  }));
  const usable = evaluated.filter(
    ({ compatibility }) => compatibility.state !== "incompatible"
  );
  if (usable.length === 0) {
    return {
      options: [],
      state: "incompatible",
      reason:
        evaluated.find(({ compatibility }) => compatibility.reason)
          ?.compatibility.reason ?? "与当前组合不兼容",
    };
  }

  const manual = usable.find(
    ({ compatibility }) => compatibility.state === "manualOnly"
  );
  return {
    options: usable.map(({ option, compatibility }) => {
      if (compatibility.state === "available") {
        return { ...option, compatibility: "available" };
      }
      return {
        ...option,
        capability: { ...option.capability, install: false },
        manualReason: compatibility.reason,
        compatibility: "manualOnly",
      };
    }),
    state: manual ? "manualOnly" : "available",
    reason: manual?.compatibility.reason ?? "",
  };
}

export function mainLoaderConflict(
  selected: LoaderSelection | null,
  candidate: LoaderId
): string {
  if (!selected || !isMainLoaderId(candidate) || selected.id === candidate) {
    return "";
  }
  if (selected.id === "optifine") {
    return "先移除 OptiFine，再选择主加载器";
  }
  return `与已选 ${LOADER_LABELS[selected.id]} 互斥`;
}

/** 下载入口的最后一道纯逻辑校验；UI 之外也必须拒绝互斥/共装组合。 */
export function downloadSelectionError(
  selections: readonly LoaderSelection[]
): string {
  const active = selections.filter((selection) => selection.version.trim());
  const main = active.filter((selection) => isMainLoaderId(selection.id));
  const uniqueMain = main.filter(
    (selection, index) =>
      main.findIndex((item) => item.id === selection.id) === index
  );
  if (uniqueMain.length > 1) {
    return `不能同时安装 ${uniqueMain
      .map((selection) => LOADER_LABELS[selection.id])
      .join("、")}；请只保留一个主加载器。`;
  }
  const optifine = active.filter((selection) => selection.id === "optifine");
  if (optifine.length > 1) return "不能重复选择 OptiFine。";
  if (uniqueMain.length > 0 && optifine.length > 0) {
    return "当前安装链只接受一个加载器，无法自动共装 OptiFine；请另存后手动放入 mods。";
  }
  return "";
}

export function resolvedDownloadIdentity(
  selection: LoaderSelection | null
): DownloadIdentity {
  return selection
    ? { loader: selection.id, loaderVersion: selection.version }
    : { loader: "", loaderVersion: null };
}
