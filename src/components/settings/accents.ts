import type { AccentColor, SourcePolicy, ThemeMode } from "./types";

/**
 * 颜色主题色板（本项目说的「主题」＝ accent，与明暗模式无关）。
 * 顺序 = 白名单顺序 = 设置页展示顺序，mono（黑与白）置顶且为默认。
 *
 * `color` 必须与 src/styles/tokens.css 中同一 accent 的 `--accent` 逐字一致
 * （mono 取暗色模式的 --accent 值）。这是全项目唯一允许出现的硬编码色，
 * 且仅用于色板预览；真正生效的仍是由 var(--accent) 令牌在运行时决定。
 */
export const ACCENT_OPTIONS: {
  value: AccentColor;
  label: string;
  color: string;
}[] = [
  // mono 的圆点不是纯色块，而是黑白分割的双色块（见 settings.css 的 .st-swatch--mono）。
  { value: "mono", label: "黑与白", color: "#f2f4f5" },
  { value: "emerald", label: "翡翠绿", color: "#3ecf8e" },
  { value: "blue", label: "海蓝色", color: "#5a97e8" },
  { value: "violet", label: "魅紫色", color: "#9b8cf0" },
  { value: "amber", label: "琥珀色", color: "#e0aa4c" },
  { value: "rose", label: "玫瑰红", color: "#ea6f8b" },
];

export const THEME_OPTIONS: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "跟随系统" },
  { value: "light", label: "亮色" },
  { value: "dark", label: "暗色" },
];

export const DOWNLOAD_SOURCE_OPTIONS: {
  value: SourcePolicy;
  label: string;
}[] = [
  { value: "mirror_first", label: "尽量使用镜像源" },
  { value: "official_auto", label: "优先使用官方源,在加载缓慢时换用镜像源" },
  { value: "official_first", label: "尽量使用官方源" },
];
