/** 显示名由文件名派生；同组重名时必须整组回退完整词干。 */

const LOADER_WORDS = new Set([
  "forge",
  "neoforge",
  "fabric",
  "quilt",
  "liteloader",
  "optifine",
]);

export function fileStem(filename: string): string {
  const trimmed = filename.trim();
  const index = trimmed.lastIndexOf(".");
  if (index <= 0 || index === trimmed.length - 1) return trimmed;
  return trimmed.slice(0, index);
}

/** 重名检测由调用方按分组执行，空结果回退完整词干。 */
export function deriveFileDisplayName(params: {
  filename: string;
  versionNumber: string;
  gameVersions: readonly string[];
}): string {
  const stem = fileStem(params.filename);
  const drop = new Set<string>();
  const version = params.versionNumber.trim().toLowerCase();
  if (version) drop.add(version);
  for (const gameVersion of params.gameVersions) {
    const tag = gameVersion.trim().toLowerCase();
    if (tag) drop.add(tag);
  }
  const words = stem
    .split(/[-_]+/)
    .filter((word) => word.length > 0)
    .filter((word) => {
      const lower = word.toLowerCase();
      return !drop.has(lower) && !LOADER_WORDS.has(lower);
    });
  return words.length > 0 ? words.join("-") : stem;
}

/** 解析失败返回空字符串，由调用方选择显示回退。 */
export function formatRelativeTime(value: string, now = Date.now()): string {
  const trimmed = value.trim();
  if (!trimmed) return "";
  const timestamp = Date.parse(trimmed);
  if (!Number.isFinite(timestamp)) return "";
  const seconds = Math.max(0, Math.floor((now - timestamp) / 1000));
  if (seconds < 60) return "刚刚";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} 天前`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months} 个月前`;
  return `${Math.floor(months / 12)} 年前`;
}
