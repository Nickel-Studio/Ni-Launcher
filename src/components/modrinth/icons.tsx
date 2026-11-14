/**
 * Modrinth 社区资源 · 图标集。
 *
 * 类型 `IconProps` 复用 `components/ui/icons`，`base` 基元与 UI 库同源（该基元未导出，
 * 与 `components/resources/icons` 同样的做法在此本地定义）。
 * 这里只补检索特有的图标，不重复 UI 库已有图标。
 */

import type { IconProps } from "../ui/icons";

const BASE: IconProps = {
  viewBox: "0 0 24 24",
  width: "1em",
  height: "1em",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.75,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
  focusable: false,
};

function base(p: IconProps): IconProps {
  return { ...BASE, ...p };
}

/** 搜索：放大镜。 */
export function IconSearch(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="11" cy="11" r="6.2" />
      <path d="m15.6 15.6 4.4 4.4" />
    </svg>
  );
}
