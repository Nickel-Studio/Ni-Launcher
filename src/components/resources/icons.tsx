/**
 * 资源中心 · 图标集。
 *
 * 类型 `IconProps` 复用 `components/ui/icons`，保证与 UI 库图标同源；
 * `base` 基元在 UI 库中未导出，故在此本地定义同样的默认属性。
 * 这里只补资源中心特有的分类与操作图标，不重复 UI 库已有图标。
 */

import type { ComponentType } from "react";
import type { IconProps } from "../ui/icons";
import type { ResourceKind } from "./types";

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

/** 全部：四宫格。 */
export function IconAll(p: IconProps) {
  return (
    <svg {...base(p)}>
      <rect x="3.5" y="3.5" width="7.1" height="7.1" rx="2.1" />
      <rect x="13.4" y="3.5" width="7.1" height="7.1" rx="2.1" />
      <rect x="3.5" y="13.4" width="7.1" height="7.1" rx="2.1" />
      <rect x="13.4" y="13.4" width="7.1" height="7.1" rx="2.1" />
    </svg>
  );
}

/** 模组：带凸点的积木块。 */
export function IconMods(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M7.6 9.6V7.1a1.6 1.6 0 0 1 3.2 0v2.5" />
      <path d="M13.2 9.6V7.1a1.6 1.6 0 0 1 3.2 0v2.5" />
      <rect x="3.6" y="9.6" width="16.8" height="10.2" rx="2.2" />
    </svg>
  );
}

/** 资源包：带背框的图片卡。 */
export function IconPack(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M20.4 8.4v9.4a2.3 2.3 0 0 1-2.3 2.3H8.4" />
      <rect x="3.4" y="3.4" width="13.3" height="13.3" rx="2.8" />
      <circle cx="7.7" cy="7.8" r="1.25" fill="currentColor" stroke="none" />
      <path d="M3.8 14.4 8.1 10.5l4.5 3.9" />
    </svg>
  );
}

/** 光影：带光芒的太阳。 */
export function IconShader(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="12" r="3.9" />
      <path d="M12 2.9v2.5" />
      <path d="M12 18.6v2.5" />
      <path d="M2.9 12h2.5" />
      <path d="M18.6 12h2.5" />
      <path d="m5.7 5.7 1.8 1.8" />
      <path d="m16.5 16.5 1.8 1.8" />
      <path d="m18.3 5.7-1.8 1.8" />
      <path d="m7.5 16.5-1.8 1.8" />
    </svg>
  );
}

/** 存档：世界（经纬网格球）。 */
export function IconWorld(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M3.5 12h17" />
      <path d="M12 3.5c2.6 2.8 2.6 14.2 0 17" />
      <path d="M12 3.5c-2.6 2.8-2.6 14.2 0 17" />
    </svg>
  );
}

/** 整合包：堆叠图层。 */
export function IconBundle(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.2 3.4 7.4 12 11.6l8.6-4.2L12 3.2Z" />
      <path d="m3.4 12.2 8.6 4.2 8.6-4.2" />
      <path d="m3.4 16.8 8.6 4.2 8.6-4.2" />
    </svg>
  );
}

/** 打开所在文件夹：目录 + 外指箭头。 */
export function IconFolderOpen(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M3.4 7.4a2 2 0 0 1 2-2h3.4l1.7 2.2h6.1a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5.4a2 2 0 0 1-2-2Z" />
      <path d="M11.6 14.6h4.8" />
      <path d="m14.6 12.8 1.8 1.8-1.8 1.8" />
    </svg>
  );
}

/** 修改时间：时钟。 */
export function IconClock(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="12" r="8.4" />
      <path d="M12 7.4V12l3.2 1.9" />
    </svg>
  );
}

/** 提醒：感叹号三角。 */
export function IconAlert(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.8 21 19.6H3Z" />
      <path d="M12 9.8v4.2" />
      <circle cx="12" cy="16.9" r="1" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** 种类 → 卡片角标图标。 */
export const KIND_ICONS: Record<ResourceKind, ComponentType<IconProps>> = {
  mods: IconMods,
  resourcepacks: IconPack,
  shaderpacks: IconShader,
  saves: IconWorld,
};
