import release from "../../assets/blocks/Release.png";
import beta from "../../assets/blocks/Beta.png";
import alpha from "../../assets/blocks/EarlyAlpha.png";
import unknown from "../../assets/blocks/Other.png";
import type { ReleaseType } from "../modrinth/types";

/** 64px 原图按 32px 显示，保留高分屏清晰度。 */
export const RELEASE_TYPES: Record<
  ReleaseType,
  { image: string; label: string }
> = {
  release: { image: release, label: "稳定版" },
  beta: { image: beta, label: "测试版" },
  alpha: { image: alpha, label: "早期测试版" },
  unknown: { image: unknown, label: "其他" },
};
