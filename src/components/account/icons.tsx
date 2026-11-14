import type { SVGProps } from "react";

/**
 * 首页本地内联图标 —— 细描边线性图标兜底。
 *
 * 刻意不依赖 `../ui/icons`：design-core 正在重做图标/配色，
 * 该模块的导出名可能变动；这里自带一份以保证本分支独立可编译。
 */
export type IconProps = SVGProps<SVGSVGElement>;

function base(p: IconProps): IconProps {
  return {
    viewBox: "0 0 24 24",
    width: "1em",
    height: "1em",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.7,
    strokeLinecap: "round",
    strokeLinejoin: "round",
    "aria-hidden": true,
    focusable: false,
    ...p,
  };
}

/** 正版账号：盾牌 + 对勾 */
export function IconShieldCheck(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3 19 6v6c0 4.4-3 7.7-7 9-4-1.3-7-4.6-7-9V6l7-3Z" />
      <path d="M9.2 12.1 11.1 14 14.9 10.1" />
    </svg>
  );
}

/** 皮肤站：图片 */
export function IconImage(p: IconProps) {
  return (
    <svg {...base(p)}>
      <rect x="3.6" y="4.8" width="16.8" height="14.4" rx="2.6" />
      <circle cx="8.9" cy="9.7" r="1.5" />
      <path d="M4.6 17.4 9.7 12.3l4.1 4.1 2.3-2.3 3.3 3.3" />
    </svg>
  );
}

/** 离线：用户 */
export function IconUser(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="8" r="3.6" />
      <path d="M4.8 20.2a7.2 7.2 0 0 1 14.4 0" />
    </svg>
  );
}

/** 启动：实心三角 */
export function IconPlay(p: IconProps) {
  return (
    <svg {...base(p)} fill="currentColor" stroke="none">
      <path d="M8.2 5.4 18.4 12 8.2 18.6V5.4Z" />
    </svg>
  );
}

/** 版本选择：层叠 */
export function IconLayers(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.2 3.4 7.6l8.6 4.4 8.6-4.4L12 3.2Z" />
      <path d="M3.4 12.3 12 16.7l8.6-4.4" />
      <path d="M3.4 16.7 12 21.1l8.6-4.4" />
    </svg>
  );
}

/** 版本设置：滑杆 */
export function IconSliders(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M5 6.5h5" />
      <path d="M14.5 6.5H19" />
      <circle cx="12.2" cy="6.5" r="2.1" />
      <path d="M5 17.5h4.5" />
      <path d="M14 17.5h5" />
      <circle cx="11.7" cy="17.5" r="2.1" />
    </svg>
  );
}

/** 显示密码：睁眼 */
export function IconEye(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M2.8 12s3.4-5.2 9.2-5.2S21.2 12 21.2 12s-3.4 5.2-9.2 5.2S2.8 12 2.8 12Z" />
      <circle cx="12" cy="12" r="2.6" />
    </svg>
  );
}

/** 隐藏密码：闭眼 */
export function IconEyeOff(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M4.2 5.4 19.8 18.6" />
      <path d="M9.6 7.1A9.6 9.6 0 0 1 12 6.8c5.8 0 9.2 5.2 9.2 5.2a15 15 0 0 1-3 3.5" />
      <path d="M6.2 9A15.6 15.6 0 0 0 2.8 12s3.4 5.2 9.2 5.2c1.1 0 2.1-.2 3-.5" />
      <path d="M10.4 10.4a2.3 2.3 0 0 0 3.2 3.2" />
    </svg>
  );
}

/** 密码：锁（皮肤站两阶段登录的第二阶段） */
export function IconLock(p: IconProps) {
  return (
    <svg {...base(p)}>
      <rect x="5" y="10.4" width="14" height="9.6" rx="2.4" />
      <path d="M8.4 10.4V7.8a3.6 3.6 0 0 1 7.2 0v2.6" />
      <path d="M12 14.2v2.2" />
    </svg>
  );
}
