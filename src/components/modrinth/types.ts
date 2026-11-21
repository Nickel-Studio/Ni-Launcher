/**
 * Modrinth 社区资源 · 数据模型。
 *
 * 与后端 `modrinth.rs` 的命令契约一字不差（命令返回归一化 JSON 文本，字段 camelCase）：
 * - `modrinth_search`           → `{"total":n,"hits":[...]}`
 * - `modrinth_project_versions` → `[{id,name,versionNumber,loaders,gameVersions,datePublished,files:[...]}]`
 * - `modrinth_install`          → taskId（进度经 `install-event`，与版本安装同源）
 * - `modrinth_open_project`     → 打开 modrinth.com 项目页
 */

/** 后端契约的资源种类（`modrinth_search` 的 `kind` 参数取值集合）。 */
export type ModrinthKind =
  | "mod"
  | "modpack"
  | "datapack"
  | "resourcepack"
  | "shader";

/** 检索分类定义（顺序即侧边栏「社区资源」分组的顺序）。 */
export interface ModrinthCategoryDef {
  id: ModrinthKind;
  label: string;
  /** 普通资源的实例子目录；整合包创建新实例，因此为 null。 */
  folder: string | null;
}

/** 分类 → 实例子目录；modpack 使用独立的整合包安装命令。 */
export const MODRINTH_CATEGORIES: readonly ModrinthCategoryDef[] = [
  { id: "mod", label: "Mod", folder: "mods" },
  { id: "modpack", label: "整合包", folder: null },
  { id: "datapack", label: "数据包", folder: "datapacks" },
  { id: "resourcepack", label: "资源包", folder: "resourcepacks" },
  { id: "shader", label: "光影包", folder: "shaderpacks" },
];

/** 命中条目的检索来源（合并检索 / 来源徽标共用）。 */
export type HitSource = "modrinth" | "curseforge";

/** 搜索结果单条。 */
export interface ModrinthHit {
  slug: string;
  title: string;
  description: string;
  iconUrl: string;
  downloads: number;
  author: string;
  projectType: string;
  versions: string[];
  loaders: string[];
  dateModified: string;
  /** 平台项目 id；与 slug 分开保存，前置去重与身份校验不能靠显示名。 */
  projectId?: string;
  /** 该条命中的检索来源（来源徽标 / 合并排序消费）。 */
  source: HitSource;
}

/** CurseForge 命中：安装链路要求数字项目 id。 */
export type CurseforgeHit = ModrinthHit & { projectId: string };

/** `modrinth_search` 归一化后的结果。 */
export interface ModrinthSearchResult {
  total: number;
  hits: ModrinthHit[];
}

/** 单个版本的一个可下载文件。 */
export interface ModrinthVersionFile {
  url: string;
  filename: string;
  sizeBytes: number;
  primary: boolean;
  hashSha1: string;
}

/** 仅声明必要关系；文件名不是项目或版本标识，不能据此猜测下载来源。 */
export interface ProjectDependency {
  dependencyType: "required";
  projectId: string;
  versionId: string;
  fileName: string;
}

export interface DependencyLookup {
  projectId?: string;
  versionId?: string;
}

export interface ResolvedDependency {
  project: ModrinthHit;
  /** null 表示上游只声明项目，绝不能用最新版本冒充要求版本。 */
  version: ModrinthVersion | null;
}

/** 单个项目版本。 */
export type ReleaseType = "release" | "beta" | "alpha" | "unknown";

export interface ModrinthVersion {
  id: string;
  projectId?: string;
  name: string;
  versionNumber: string;
  loaders: string[];
  gameVersions: string[];
  datePublished: string;
  releaseType: ReleaseType;
  files: ModrinthVersionFile[];
  /** undefined = 旧响应未提供元数据；空数组 = 已确认没有必要前置。 */
  dependencies?: ProjectDependency[];
}

export interface ProjectVersionPage {
  versions: ModrinthVersion[];
  offset: number;
  nextOffset: number;
  hasMore: boolean;
  total: number | null;
}

/** 社区资源检索的过滤条件（来自当前激活实例）。 */
export interface ModrinthFilter {
  /** MC 版本号（如 `1.20.1`）；空串表示未识别、不做版本过滤。 */
  mcVersion: string;
  /** 加载器标识（vanilla / forge / neoforge / fabric / optifine / …）。 */
  loader: string;
}
