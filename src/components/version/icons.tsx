import type { IconProps } from "../ui/icons";

/**
 * 版本页专用图标。
 *
 * 与 `components/ui/icons.tsx` 同一绘制风格（24 网格、1.75 描边、currentColor），
 * 但刻意收在版本模块内：目录/告警/返回图标只服务于本页，不必让 ui 基元包或
 * 其它业务模块承担这个依赖。
 */

/** 目录（实例根目录）。 */
export function IconFolder(p: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      focusable={false}
      {...p}
    >
      <path d="M3.2 7.6a2 2 0 0 1 2-2h3.2l1.9 2.1h8.5a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5.2a2 2 0 0 1-2-2V7.6Z" />
      <path d="M3.2 9.8h17.6" />
    </svg>
  );
}

/** 告警三角（错误状态块用）。 */
export function IconAlert(p: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      focusable={false}
      {...p}
    >
      <path d="M12 4.2 21 19.6H3L12 4.2Z" />
      <path d="M12 10v4.4" />
      <circle cx="12" cy="16.8" r="0.95" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** 返回（独立页头部「返回首页」入口用）。 */
export function IconArrowLeft(p: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      focusable={false}
      {...p}
    >
      <path d="M10.4 5.6 4 12l6.4 6.4" />
      <path d="M4 12h16" />
    </svg>
  );
}

/** 下载（「去下载」入口按钮用）。 */
export function IconDownloadArrow(p: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      focusable={false}
      {...p}
    >
      <path d="M12 4.4v10.4" />
      <path d="M7.4 10.4 12 15l4.6-4.6" />
      <path d="M4.6 19.2h14.8" />
    </svg>
  );
}

/** 滑杆（实例卡片「版本设置」动作用）。 */
export function IconSliders(p: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      focusable={false}
      {...p}
    >
      <path d="M4 7h10" />
      <path d="M18.2 7H20" />
      <circle cx="16.1" cy="7" r="2.1" />
      <path d="M4 17h4" />
      <path d="M12.2 17H20" />
      <circle cx="10.1" cy="17" r="2.1" />
    </svg>
  );
}
