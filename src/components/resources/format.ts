/**
 * 资源中心 · 展示格式化（纯函数）。
 * 只做「数值 → 文案」的转换，不读取任何全局状态。
 */

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** 字段缺失时统一使用的占位符。 */
export const EMPTY_VALUE = "—";

/** 文件大小 → 人类可读文案；后端未提供大小时显示占位符。 */
export function formatBytes(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0)
    return EMPTY_VALUE;
  if (bytes < 1024) return `${bytes} B`;

  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }

  // 三位数以内保留一位小数（如 12.6 MB），再大则省去小数（如 512 MB）。
  const text = value < 100 ? value.toFixed(1) : value.toFixed(0);
  return `${text} ${BYTE_UNITS[unit]}`;
}

const DATE_FORMATTER = new Intl.DateTimeFormat("zh-CN", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});

/** Unix 秒 → 本地日期时间文案；后端未提供时间时显示占位符。 */
export function formatTimestamp(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return EMPTY_VALUE;
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return EMPTY_VALUE;
  return DATE_FORMATTER.format(date);
}

const ISO_DATE_FORMATTER = new Intl.DateTimeFormat("zh-CN", {
  year: "numeric",
  month: "long",
  day: "numeric",
});

/** RFC3339 时间字符串 → 本地化日期（如「2024年6月13日」）；无法解析时回退日期部分。 */
export function formatIsoDate(raw: string | null): string {
  if (!raw) return EMPTY_VALUE;
  const time = Date.parse(raw);
  if (!Number.isFinite(time)) return raw.slice(0, 10) || EMPTY_VALUE;
  return ISO_DATE_FORMATTER.format(new Date(time));
}
