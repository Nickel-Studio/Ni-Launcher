import type { ReactNode } from "react";
import "./resources.css";

interface NoticeProps {
  tone?: "neutral" | "error";
  icon: ReactNode;
  title: string;
  hint: string;
  /** 补充的真实细节（如实例目录、失败原因），可为空。 */
  detail?: string;
  actions?: ReactNode;
}

/** 空状态 / 错误状态占位块：图标 + 标题 + 可执行提示。 */
export function ResourceNotice({
  tone = "neutral",
  icon,
  title,
  hint,
  detail,
  actions,
}: NoticeProps) {
  return (
    <div
      className={`rs-notice rs-notice--${tone}`}
      role={tone === "error" ? "alert" : undefined}
    >
      <div className="rs-notice__icon" aria-hidden>
        {icon}
      </div>
      <h2 className="rs-notice__title">{title}</h2>
      <p className="rs-notice__hint">{hint}</p>
      {detail ? <p className="rs-notice__detail">{detail}</p> : null}
      {actions ? <div className="rs-notice__actions">{actions}</div> : null}
    </div>
  );
}

interface SkeletonProps {
  /** 骨架卡片数量。 */
  count?: number;
}

/** 扫描中的骨架网格（形状与真实卡片一致，避免布局跳动）。 */
export function ResourceSkeletonGrid({ count = 8 }: SkeletonProps) {
  return (
    <div className="rs-grid" aria-hidden="true">
      {Array.from({ length: count }, (_, index) => (
        <div key={index} className="rs-skel nw-card">
          <span className="rs-skel__bar rs-skel__bar--sm" />
          <span className="rs-skel__bar rs-skel__bar--lg" />
          <span className="rs-skel__bar" />
          <span className="rs-skel__bar rs-skel__bar--sm" />
        </div>
      ))}
    </div>
  );
}
