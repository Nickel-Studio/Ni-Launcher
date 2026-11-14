/**
 * 版本实例 · 后端 IPC 与返回值归一化。
 *
 * 后端以「JSON 文本」返回数据（`instances_get` / `instance_scan`），
 * 因此这里统一做：安全解析 → 类型收敛 → 中文错误翻译。
 * 调用点只负责「调用 + 归一化」，失败时抛出可展示的错误，由上层决定降级呈现，
 * 绝不让界面白屏。
 *
 * 契约（与 Rust `instances.rs` 一字不差）：
 * - `instances_get`   → `{"roots":["<目录>", …], "active":"<目录>" | null}`
 * - `instances_set`   → 纯 JSON 文本透传，>256KB 或非法 JSON 会 Err
 * - `instance_scan`   → `[{ id, type, loader, loaderVersion, icon, path, releaseTime, hasJson }]`
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";

/** 实例根目录注册表：`roots` 是 `.minecraft` 目录列表，`active` 为当前目录。 */
export interface InstanceRegistry {
  roots: string[];
  active: string;
  labels: Record<string, string>;
}

/** 一个版本实例（`<root>/versions/<id>`）。 */
export interface InstanceEntry {
  id: string;
  displayName: string;
  favorite: boolean;
  favoriteAt: number | null;
  category: string;
  /** 版本类型：release / snapshot / old_beta / old_alpha / unknown。 */
  type: string;
  /** 加载器：vanilla / forge / neoforge / fabric / optifine / liteloader / optifabric / legacy / unknown。 */
  loader: string;
  loaderVersion: string | null;
  minecraftVersion: string | null;
  namedProfile: boolean;
  /** Blocks 图标键：grass / command_block / cobblestone / anvil / neoforge / fabric / grass_path / egg / gold_block / redstone_block。 */
  icon: string;
  /** 实例目录绝对路径。 */
  path: string;
  releaseTime: string | null;
  /** 版本 JSON 是否可解析；false 表示无法解析、也就无法启动。 */
  hasJson: boolean;
}

export const EMPTY_REGISTRY: InstanceRegistry = {
  roots: [],
  active: "",
  labels: {},
};

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

/** 去掉尾部分隔符，便于比较与展示（同时兼容 `\\` 与 `/`）。 */
function trimSeparators(dir: string): string {
  return dir.replace(/[\\/]+$/, "");
}

/**
 * 归一化 `instances_get` 返回值。
 * `roots` 按契约是字符串数组；同时容忍 `{ dir }` 对象写法与重复项，
 * 避免一份残缺/历史数据导致整个实例列表丢失。
 */
export function normalizeRegistry(raw: unknown): InstanceRegistry {
  const data = toData(raw);
  if (!data || typeof data !== "object") return { ...EMPTY_REGISTRY };
  const record = data as Record<string, unknown>;

  const roots: string[] = [];
  const seen = new Set<string>();
  const rawRoots = Array.isArray(record.roots) ? record.roots : [];
  for (const item of rawRoots) {
    const candidate =
      typeof item === "string"
        ? item
        : item && typeof item === "object"
        ? String((item as Record<string, unknown>).dir ?? "")
        : "";
    const dir = trimSeparators(candidate.trim());
    if (!dir || seen.has(dir)) continue;
    seen.add(dir);
    roots.push(dir);
  }

  const active =
    typeof record.active === "string"
      ? trimSeparators(record.active.trim())
      : "";
  const labels: Record<string, string> = {};
  if (record.labels && typeof record.labels === "object") {
    for (const [dir, label] of Object.entries(record.labels)) {
      if (typeof label === "string") labels[dir] = label;
    }
  }
  return { roots, active, labels };
}

/** 序列化注册表；`active` 为空时写 `null`，与后端 `Option<String>` 对齐。 */
export function registryToText(registry: InstanceRegistry): string {
  return JSON.stringify({
    roots: registry.roots,
    active: registry.active || null,
    labels: registry.labels,
  });
}

/** 读取实例注册表。 */
export async function loadRegistry(): Promise<InstanceRegistry> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("instances_get");
  return normalizeRegistry(raw);
}

/** 写入实例注册表（纯 JSON 文本透传，后端做大小与合法性校验）。 */
export async function saveRegistry(registry: InstanceRegistry): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("instances_set", { json: registryToText(registry) });
}

/**
 * 确保「启动器自身目录下的 .minecraft」存在并注册为实例目录，返回其路径。
 * 这是首屏注册表为空时的自动初始化入口。
 */
export async function ensureDefaultInstance(): Promise<string> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const dir = await invoke<string>("instance_ensure_default");
  return typeof dir === "string" ? trimSeparators(dir.trim()) : "";
}

/**
 * 启动器自带 / 便携包定义的游戏目录（不读取 `game.json`，也不会创建目录）。
 * 只用于把该目录的默认展示名设为「默认实例目录」，物理目录名仍是 `.minecraft`。
 */
export async function fetchDefaultInstanceDir(): Promise<string> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const dir = await invoke<string>("instance_default_dir");
  return typeof dir === "string" ? trimSeparators(dir.trim()) : "";
}

/**
 * 实例取名优化：后端只在「孤立的 `.minecraft`」时返回上级目录名。
 * 其它情况（同级已有多个实例、用户目录 / 系统目录、根目录）一律返回 `null`。
 */
export async function suggestInstanceLabel(
  dir: string
): Promise<string | null> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const label = await invoke<string | null>("instance_suggest_label", { dir });
  return typeof label === "string" && label.trim() ? label.trim() : null;
}

/** 写入实例目录的展示名；只改注册表标签，不移动磁盘目录。 */
export async function renameInstanceRoot(
  dir: string,
  label: string
): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("instance_root_rename", { dir, label });
}

/**
 * 归一化 `instance_scan` 返回值。
 * 返回值不是数组时抛错（上层展示「扫描不可用」），
 * 单条数据缺字段则跳过，避免一条脏数据污染整张卡片网格。
 */
export function normalizeEntries(raw: unknown): InstanceEntry[] {
  const data = toData(raw);
  if (!Array.isArray(data)) throw new Error("实例扫描返回的数据格式无法识别");

  const entries: InstanceEntry[] = [];
  for (const item of data) {
    if (!item || typeof item !== "object") continue;
    const record = item as Record<string, unknown>;
    const id = typeof record.id === "string" ? record.id.trim() : "";
    const path = typeof record.path === "string" ? record.path.trim() : "";
    if (!id && !path) continue;
    const loaderVersion =
      typeof record.loaderVersion === "string" && record.loaderVersion.trim()
        ? record.loaderVersion.trim()
        : null;
    const releaseTime =
      typeof record.releaseTime === "string" && record.releaseTime.trim()
        ? record.releaseTime.trim()
        : null;
    entries.push({
      id: id || path,
      displayName:
        typeof record.displayName === "string" && record.displayName.trim()
          ? record.displayName.trim()
          : id || path,
      type: typeof record.type === "string" ? record.type : "unknown",
      loader: typeof record.loader === "string" ? record.loader : "unknown",
      loaderVersion,
      minecraftVersion:
        typeof record.minecraftVersion === "string"
          ? record.minecraftVersion
          : null,
      namedProfile: record.namedProfile === true,
      favorite: record.favorite === true,
      favoriteAt:
        typeof record.favoriteAt === "number" &&
        Number.isFinite(record.favoriteAt)
          ? record.favoriteAt
          : null,
      category: typeof record.category === "string" ? record.category : "",
      icon:
        typeof record.icon === "string" && record.icon.trim()
          ? record.icon.trim()
          : "redstone_block",
      path,
      releaseTime,
      hasJson: record.hasJson === true,
    });
  }

  // 收藏时间由后端持久化；旧收藏无时间戳时维持稳定的目录身份排序。
  entries.sort((a, b) => {
    if (a.favorite !== b.favorite) return a.favorite ? -1 : 1;
    if (a.favorite && b.favorite) {
      const order = (a.favoriteAt ?? 0) - (b.favoriteAt ?? 0);
      if (order !== 0) return order;
    }
    if (a.hasJson !== b.hasJson) return a.hasJson ? -1 : 1;
    return b.id.localeCompare(a.id, undefined, { numeric: true });
  });
  return entries;
}

/** 扫描某个 `.minecraft` 根目录下的版本实例。 */
export async function scanInstances(dir: string): Promise<InstanceEntry[]> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("instance_scan", { dir });
  return normalizeEntries(raw);
}

/** 从任意错误值里取出可展示的文案。 */
function rawMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "";
}

/** 把 IPC / 文件失败原因翻译成可执行的中文提示。 */
export function describeInstancesError(err: unknown): string {
  const message = rawMessage(err);
  if (/not found|unknown command|not_allowed|no such/i.test(message)) {
    return "当前构建未提供实例相关命令，无法读写实例目录。";
  }
  if (/permission|denied|forbidden|access is denied/i.test(message)) {
    return "系统拒绝了访问该目录的请求，请检查目录权限后重试。";
  }
  return message || "实例操作失败，请重试。";
}
