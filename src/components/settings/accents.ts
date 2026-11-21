import { THEME_COLORS } from "../../lib/theme";
import type { AccentColor, SourcePolicy, ThemeMode } from "./types";

/** Palette order is also the persisted theme whitelist order. */
export const ACCENT_OPTIONS: {
  value: AccentColor;
  label: string;
  color: string;
  description?: string;
}[] = [
  { value: "mono", label: "黑与白", color: THEME_COLORS.mono },
  { value: "emerald", label: "翡翠绿", color: THEME_COLORS.emerald },
  { value: "blue", label: "海蓝色", color: THEME_COLORS.blue },
  { value: "violet", label: "魅紫色", color: THEME_COLORS.violet },
  { value: "amber", label: "琥珀色", color: THEME_COLORS.amber },
  { value: "rose", label: "玫瑰红", color: THEME_COLORS.rose },
  {
    value: "nickel",
    label: "镍光",
    color: THEME_COLORS.nickel,
  },
  {
    value: "copper",
    label: "铜霞",
    color: THEME_COLORS.copper,
  },
  {
    value: "abyss",
    label: "深海",
    color: THEME_COLORS.abyss,
  },
  {
    value: "paper",
    label: "纸灯",
    color: THEME_COLORS.paper,
  },
  {
    value: "fortyTwo",
    label: "四十二",
    color: THEME_COLORS.fortyTwo,
  },
  {
    value: "custom",
    label: "自定义",
    color: "var(--theme-ink, var(--accent))",
    description: "调整颜色与顶部装饰",
  },
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
