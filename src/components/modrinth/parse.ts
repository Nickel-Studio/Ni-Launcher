/**
 * Modrinth 社区资源 · 后端返回值归一化（纯函数）。
 *
 * 后端一律以「JSON 文本」返回清单类数据，字段已由后端收敛为 camelCase；
 * 这里再做一道防御性收敛：任何残缺数据都丢弃或回退为空值，绝不抛出未捕获异常。
 */
import { toData } from "../resources/parse";
import type {
  CurseforgeHit,
  HitSource,
  ModrinthHit,
  ModrinthSearchResult,
  ModrinthVersion,
  ModrinthVersionFile,
  ProjectDependency,
  ResolvedDependency,
} from "./types";

type RawPayload = Record<string, unknown>;

/** 取字符串字段（缺失 / 类型不符时回退空串）。 */
function pickString(value: RawPayload, key: string): string {
  const raw = value[key];
  return typeof raw === "string" ? raw.trim() : "";
}

/** 取数字字段（缺失 / 非有限数字时回退 0）。 */
function pickNumber(value: RawPayload, key: string): number {
  const raw = value[key];
  return typeof raw === "number" && Number.isFinite(raw) ? raw : 0;
}

/** 取字符串数组字段（非数组回退空数组）。 */
function pickStringArray(value: RawPayload, key: string): string[] {
  const raw = value[key];
  if (!Array.isArray(raw)) return [];
  return raw.filter((item): item is string => typeof item === "string");
}

/**
 * 归一化 `modrinth_search` 返回值（缺 slug 的脏数据丢弃）。
 * `source` 由调用点传入并打进每条命中，来源徽标与合并检索都消费它。
 */
export function normalizeSearch(
  raw: unknown,
  source: HitSource
): ModrinthSearchResult {
  const data = toData(raw);
  if (!data || typeof data !== "object") return { total: 0, hits: [] };
  const record = data as RawPayload;

  const hits: ModrinthHit[] = [];
  const rawHits = Array.isArray(record.hits) ? record.hits : [];
  for (const item of rawHits) {
    if (!item || typeof item !== "object") continue;
    const entry = item as RawPayload;
    const slug = pickString(entry, "slug");
    if (!slug) continue;
    hits.push({
      slug,
      title: pickString(entry, "title") || slug,
      description: pickString(entry, "description"),
      iconUrl: pickString(entry, "iconUrl"),
      downloads: pickNumber(entry, "downloads"),
      author: pickString(entry, "author"),
      projectType: pickString(entry, "projectType"),
      versions: pickStringArray(entry, "versions"),
      loaders: pickStringArray(entry, "loaders"),
      dateModified: pickString(entry, "dateModified"),
      projectId: pickString(entry, "projectId"),
      source,
    });
  }

  const total = pickNumber(record, "total");
  return { total: total > 0 ? total : hits.length, hits };
}

/** 合并两源命中：按来源与稳定项目标识去重，跨源同名项目保留。 */
export function mergeHits(
  primary: ModrinthHit[],
  secondary: ModrinthHit[]
): ModrinthHit[] {
  const byProject = new Map<string, ModrinthHit>();
  for (const hit of [...primary, ...secondary]) {
    const identity = `${hit.source}:${hit.projectId || hit.slug}`;
    const existing = byProject.get(identity);
    if (!existing || hit.downloads > existing.downloads) {
      byProject.set(identity, hit);
    }
  }
  return [...byProject.values()].sort((a, b) => b.downloads - a.downloads);
}

/** CF 命中守卫：安装链路要求数字项目 id（后端已保证存在，此处兜底脏数据）。 */
export function withProjectId(hit: ModrinthHit): CurseforgeHit | null {
  return hit.projectId ? { ...hit, projectId: hit.projectId } : null;
}

/** 归一化单个文件条目；缺少下载地址的文件丢弃。 */
function normalizeFile(raw: unknown): ModrinthVersionFile | null {
  if (!raw || typeof raw !== "object") return null;
  const entry = raw as RawPayload;
  const url = pickString(entry, "url");
  if (!url) return null;
  return {
    url,
    filename: pickString(entry, "filename"),
    sizeBytes: pickNumber(entry, "sizeBytes"),
    primary: entry.primary === true,
    hashSha1: pickString(entry, "hashSha1"),
  };
}

function dependencyId(value: unknown): string {
  if (typeof value === "string") return value.trim();
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? String(value)
    : "";
}

/** 两源只接受明确的必要关系；可选、不兼容和嵌入关系不参与检测。 */
export function normalizeDependencies(raw: unknown): ProjectDependency[] {
  const data = toData(raw);
  if (!Array.isArray(data)) throw new Error("前置元数据格式无效，请重试。");
  return data.flatMap((item): ProjectDependency[] => {
    if (!item || typeof item !== "object") return [];
    const entry = item as RawPayload;
    const type = entry.dependencyType ?? entry.dependency_type;
    const required =
      entry.relationType === undefined
        ? type === "required"
        : entry.relationType === 3 &&
          (type === undefined || type === "required");
    if (!required) return [];
    return [
      {
        dependencyType: "required",
        projectId: dependencyId(
          entry.projectId ?? entry.project_id ?? entry.modId
        ),
        versionId: dependencyId(entry.versionId ?? entry.version_id),
        fileName:
          pickString(entry, "fileName") || pickString(entry, "file_name"),
      },
    ];
  });
}

/** 归一化项目版本；前置结构损坏只影响前置区，不能阻塞主版本列表。 */
export function normalizeVersions(raw: unknown): ModrinthVersion[] {
  const data = toData(raw);
  if (!Array.isArray(data)) throw new Error("版本列表格式无效，请重试。");
  const versions: ModrinthVersion[] = [];
  for (const item of data) {
    if (!item || typeof item !== "object") continue;
    const entry = item as RawPayload;
    const id = pickString(entry, "id");
    if (!id) continue;
    const release = pickString(entry, "releaseType");
    versions.push({
      id,
      projectId: pickString(entry, "projectId"),
      name: pickString(entry, "name") || pickString(entry, "versionNumber"),
      versionNumber: pickString(entry, "versionNumber"),
      loaders: pickStringArray(entry, "loaders"),
      gameVersions: pickStringArray(entry, "gameVersions"),
      datePublished: pickString(entry, "datePublished"),
      releaseType:
        release === "release" || release === "beta" || release === "alpha"
          ? release
          : "unknown",
      files: (Array.isArray(entry.files) ? entry.files : [])
        .map(normalizeFile)
        .filter((file): file is ModrinthVersionFile => file !== null),
      dependencies: Array.isArray(entry.dependencies)
        ? normalizeDependencies(entry.dependencies)
        : undefined,
    });
  }
  return versions;
}

export function normalizeDependencyResolution(
  raw: unknown,
  source: HitSource
): ResolvedDependency {
  const data = toData(raw);
  if (!data || typeof data !== "object" || Array.isArray(data))
    throw new Error("前置项目信息格式无效，请重试。");
  const record = data as RawPayload;
  const project = normalizeSearch({ hits: [record.project] }, source).hits[0];
  if (!project?.projectId) throw new Error("前置项目缺少标识，请重试。");
  const version =
    record.version == null ? null : normalizeVersions([record.version])[0];
  if (record.version != null && !version)
    throw new Error("指定前置版本不可用，请重试。");
  if (version?.projectId && version.projectId !== project.projectId)
    throw new Error("指定前置版本不属于返回的项目，请重试。");
  return { project, version: version ?? null };
}

export function normalizeVersionPage(
  raw: unknown
): import("./types").ProjectVersionPage {
  const data = toData(raw);
  if (!data || typeof data !== "object" || Array.isArray(data)) {
    throw new Error("历史版本分页格式无效，请重试。");
  }
  const record = data as RawPayload;
  const versions = normalizeVersions(record.versions);
  const offset = pickNumber(record, "offset");
  const nextOffset = pickNumber(record, "nextOffset");
  if (record.hasMore === true && nextOffset <= offset)
    throw new Error("历史版本分页游标无效，请重试。");
  return {
    versions,
    offset,
    nextOffset,
    hasMore: record.hasMore === true,
    total:
      typeof record.total === "number" && Number.isFinite(record.total)
        ? record.total
        : null,
  };
}

/** 下载量 → 「480 万」「1.2 千」；小于 1000 原样输出。 */
export function formatDownloads(count: number): string {
  if (!Number.isFinite(count) || count < 0) return "—";
  if (count >= 100_000_000) return `${trim(count / 100_000_000)} 亿`;
  if (count >= 10_000) return `${trim(count / 10_000)} 万`;
  if (count >= 1000) return `${trim(count / 1000)} 千`;
  return String(count);
}

function trim(value: number): string {
  return value >= 100
    ? String(Math.round(value))
    : value.toFixed(1).replace(/\.0$/, "");
}

/** ISO 时间字符串 → 「2026-01-02」；解析失败返回空串。 */
export function formatIsoDate(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return "";
  const date = trimmed.slice(0, 10);
  return /^\d{4}-\d{2}-\d{2}$/.test(date) ? date : "";
}
