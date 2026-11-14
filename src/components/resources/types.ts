/**
 * 资源中心 · 本地扫描数据模型。
 *
 * `ResourceKind` 与后端命令 `resources_scan` 的 `kind` 参数一一对应；
 * 页面内「下载中心」的远端模型见 `components/download/types.ts`。
 */

/** `resources_scan` 支持的种类。 */
export type ResourceKind = "mods" | "resourcepacks" | "shaderpacks" | "saves";

/** 参与本地扫描的种类，顺序即「已安装」列表内的分组顺序。 */
export const SCAN_KINDS: readonly ResourceKind[] = [
  "mods",
  "resourcepacks",
  "shaderpacks",
  "saves",
];

/** 种类 → 中文标签。 */
export const KIND_LABELS: Record<ResourceKind, string> = {
  mods: "模组",
  resourcepacks: "资源包",
  shaderpacks: "光影",
  saves: "存档",
};

/** 单条本地资源。 */
export interface ResourceEntry {
  /** 稳定 key：种类 + 绝对路径。 */
  id: string;
  kind: ResourceKind;
  name: string;
  path: string;
  /** 字节数；后端未提供时为 null。 */
  sizeBytes: number | null;
  /** 修改时间（Unix 秒）；后端未提供时为 null。 */
  modified: number | null;
  isDir: boolean;
}

/** 一个实例根目录（`.minecraft`）。 */
export interface InstanceRoot {
  dir: string;
  label: string;
}

/** `instances_get` 归一化后的结果。 */
export interface InstanceInfo {
  roots: InstanceRoot[];
  active: string;
}

/**
 * 社区资源检索的实例过滤条件（来自当前激活实例的真实数据）。
 *
 * 结构对 Modrinth / CurseForge 两个检索引擎通用：
 * `ModrinthFilter`（modrinth/types.ts）与之结构完全一致，可直接互赋。
 */
export interface InstanceSearchFilter {
  /** 真实 Minecraft 版本（如 `1.20.1` / `26.3`）；未知为空，不能用实例名补齐。 */
  mcVersion: string;
  /** 加载器标识（vanilla / forge / neoforge / fabric / optifine / …）。 */
  loader: string;
}

/** 社区筛选与安装共用快照；label 仅展示，root + versionId 才是实例身份。 */
export interface ResourceInstance extends InstanceRoot, InstanceSearchFilter {
  root: string;
  versionId: string;
}

/* ============================================================================
 * 本地扫描分类（「资源」tab 侧边栏的扫描部分）
 * ========================================================================== */

/** 侧边栏分类 id：`all` = 四类聚合；`modpacks` 本期无扫描来源（见 useResources）。 */
export type ResourceCategory =
  | "all"
  | "mods"
  | "resourcepacks"
  | "shaderpacks"
  | "modpacks";

export interface ResourceCategoryDef {
  id: ResourceCategory;
  label: string;
  /** 参与扫描的种类；空数组 = 本期没有扫描来源。 */
  kinds: readonly ResourceKind[];
}

/** 分类定义（顺序即侧边栏顺序）。 */
export const CATEGORIES: readonly ResourceCategoryDef[] = [
  { id: "all", label: "全部", kinds: SCAN_KINDS },
  { id: "mods", label: "模组", kinds: ["mods"] },
  { id: "resourcepacks", label: "资源包", kinds: ["resourcepacks"] },
  { id: "shaderpacks", label: "光影", kinds: ["shaderpacks"] },
  // 整合包以「版本隔离实例」的形式存在于实例目录中，当前版本尚未提供实例枚举。
  { id: "modpacks", label: "整合包", kinds: [] },
];
