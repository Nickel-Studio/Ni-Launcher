/**
 * 下载中心 · 图标集。
 *
 * `IconProps` 复用 `components/ui/icons`，与 UI 库图标同源；
 * 这里只补下载中心特有的图形（气泡 / 回到顶部 / 分隔等）。
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

/** 回到顶部：向上箭头 + 顶横线。 */
export function IconArrowUp(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 19.4V5.6" />
      <path d="M6.4 11.2 12 5.6l5.6 5.6" />
    </svg>
  );
}

/** 下载详情：向下的箭头落入托盘。 */
export function IconDownloadTray(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.6v10.2" />
      <path d="M7.8 9.6 12 13.8l4.2-4.2" />
      <path d="M4.8 16v2.6a1.8 1.8 0 0 0 1.8 1.8h10.8a1.8 1.8 0 0 0 1.8-1.8V16" />
    </svg>
  );
}

/** 取消：圆圈内的横线。 */
export function IconStop(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="12" r="8.4" />
      <path d="M8.6 12h6.8" />
    </svg>
  );
}
