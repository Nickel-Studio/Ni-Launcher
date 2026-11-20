import type { CSSProperties } from "react";
import logoUrl from "../assets/logo.png";
import { PRODUCT_NAME } from "../lib/brand";

/** @deprecated 仅保留旧导出兼容；品牌位置使用真实应用图标。 */
export function Sparkles({ className }: { className?: string }) {
  return (
    <span
      className={["ni-sparkles", className].filter(Boolean).join(" ")}
      aria-hidden="true"
    >
      <i className="ni-sparkles__dot ni-sparkles__dot--a" />
      <i className="ni-sparkles__dot ni-sparkles__dot--b" />
      <i className="ni-sparkles__dot ni-sparkles__dot--c" />
    </span>
  );
}

export interface BrandMarkProps {
  size?: number;
  /** 装饰用途传空字符串，避免重复朗读相邻产品名称。 */
  alt?: string;
  glow?: boolean;
  /** @deprecated 保留旧属性兼容，品牌图标不绘制额外装饰。 */
  sparkles?: boolean;
  className?: string;
}

export function BrandMark({
  size = 72,
  alt = PRODUCT_NAME,
  glow = false,
  className,
}: BrandMarkProps) {
  return (
    <span
      className={["ni-brandmark", className].filter(Boolean).join(" ")}
      style={{ "--mark-size": `${size}px` } as CSSProperties}
    >
      {glow ? <span className="ni-brandmark__glow" aria-hidden="true" /> : null}
      <img
        className="ni-brandmark__img"
        src={logoUrl}
        alt={alt}
        width={size}
        height={size}
        draggable={false}
      />
    </span>
  );
}

export default BrandMark;
