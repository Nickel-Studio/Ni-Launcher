import "./SettingHelp.css";

export const WINDOW_TITLE_HELP =
  "支持 {date} 与 {time}，游戏运行时自动刷新。清空后使用游戏默认标题。";

/**
 * 兼容旧调用：标签只渲染普通文本。
 * 说明改由 SettingRow 的 help 属性挂在控件容器上，悬浮控件时才出现，
 * 不再改变标签光标、下划线或键盘顺序。
 */
export function SettingHelp({ label }: { label: string; help?: string }) {
  return <span className="st-setting-help">{label}</span>;
}
