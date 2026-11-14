/**
 * 资源中心 · 返回值归一化与路径/错误处理（纯函数）。
 *
 * 后端以「JSON 文本」返回数据（`instances_get` / `resources_scan`），
 * 因此这里统一做：安全解析 → 类型收敛 → 丢弃残缺条目，绝不抛出未捕获异常。
 */

import {
  KIND_LABELS,
  type InstanceInfo,
  type InstanceRoot,
  type ResourceEntry,
  type ResourceKind,
} from "./types";

/** 尝试解析 JSON 文本；失败返回 null，不抛错。 */
function tryParse(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

/** 命令返回值既可能是 JSON 文本，也可能已是对象 —— 统一成 unknown 再收敛。 */
export function toData(raw: unknown): unknown {
  return typeof raw === "string" ? tryParse(raw) : raw;
}

/** 取路径最后一段作为名称（同时兼容 / 与 \\ 分隔符）。 */
export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

/** 取所在目录（同时兼容 / 与 \\ 分隔符）；无法判定时返回空串。 */
export function directoryOf(path: string): string {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (index === 0) return path.slice(0, 1);
  if (index < 0) return "";
  return path.slice(0, index);
}

/** 从任意错误值中提取可展示的文案（拿不到时返回空串，交由调用方兜底）。 */
export function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "";
}

/** 收敛数字字段；非法值或低于下限时返回 null（表示「后端未提供」）。 */
function toNumber(raw: unknown, min: number): number | null {
  if (typeof raw === "number" && Number.isFinite(raw) && raw >= min) return raw;
  if (typeof raw === "string") {
    const value = Number(raw);
    if (Number.isFinite(value) && value >= min) return value;
  }
  return null;
}

/**
 * 归一化 `instances_get` 的返回值。
 * 结构不符（或解析失败）时返回 null，调用方据此走「未检测到实例目录」降级。
 */
export function normalizeInstances(raw: unknown): InstanceInfo | null {
  const data = toData(raw);
  if (!data || typeof data !== "object") return null;
  const record = data as Record<string, unknown>;

  const roots: InstanceRoot[] = [];
  const rawRoots = Array.isArray(record.roots) ? record.roots : [];
  for (const item of rawRoots) {
    if (!item || typeof item !== "object") continue;
    const entry = item as Record<string, unknown>;
    const dir = typeof entry.dir === "string" ? entry.dir.trim() : "";
    if (!dir) continue;
    const label =
      typeof entry.label === "string" && entry.label.trim()
        ? entry.label.trim()
        : baseName(dir);
    roots.push({ dir, label });
  }

  const active = typeof record.active === "string" ? record.active.trim() : "";
  if (!active && roots.length === 0) return null;
  return { roots, active: active || roots[0].dir };
}

/**
 * 归一化 `resources_scan` 的返回值。
 * 返回值不是数组时抛错（调用方捕获后展示「扫描不可用」），
 * 单个条目字段缺失则跳过，避免一条脏数据污染整张列表。
 */
export function normalizeEntries(
  raw: unknown,
  kind: ResourceKind
): ResourceEntry[] {
  const data = toData(raw);
  if (!Array.isArray(data)) {
    throw new Error("扫描返回的数据格式无法识别");
  }

  const entries: ResourceEntry[] = [];
  for (const item of data) {
    if (!item || typeof item !== "object") continue;
    const record = item as Record<string, unknown>;
    const path = typeof record.path === "string" ? record.path.trim() : "";
    if (!path) continue;
    const rawName = typeof record.name === "string" ? record.name.trim() : "";
    entries.push({
      id: `${kind}:${path}`,
      kind,
      name: rawName || baseName(path),
      path,
      sizeBytes: toNumber(record.sizeBytes, 0),
      // modified 为 0 视为「未提供」：Unix 纪元时间不是有效修改时间。
      modified: toNumber(record.modified, 1),
      isDir: record.isDir === true,
    });
  }

  entries.sort((a, b) => a.name.localeCompare(b.name, "zh-Hans-CN"));
  return entries;
}

/** 把扫描失败原因翻译成可执行的中文提示。 */
export function describeScanError(
  err: unknown,
  kind: ResourceKind,
  dir: string
): string {
  const label = KIND_LABELS[kind];
  const message = errorMessage(err);
  if (/not found|unknown command|not_allowed|no such/i.test(message)) {
    return `${label}扫描不可用：当前构建未提供资源扫描命令`;
  }
  if (/permission|denied|forbidden|access is denied/i.test(message)) {
    return `${label}目录无访问权限：${dir}`;
  }
  return message ? `${label}扫描失败：${message}` : `${label}扫描失败`;
}

/** 把「打开所在文件夹」失败原因翻译成可执行的中文提示。 */
export function describeOpenError(err: unknown): string {
  const message = errorMessage(err);
  if (/permission|denied|forbidden|not allowed/i.test(message)) {
    return "无法打开所在文件夹：系统拒绝了该操作";
  }
  // 文件管理器接口缺失/未注册（如 opener 插件未接线）。
  if (/not found|unknown command|plugin:/i.test(message)) {
    return "无法打开所在文件夹：文件管理器接口不可用";
  }
  return message ? `无法打开所在文件夹：${message}` : "无法打开所在文件夹";
}
