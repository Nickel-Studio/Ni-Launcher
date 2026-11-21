import type { HTMLAttributes, ReactNode } from "react";
import "./InlineNotice.css";

export type InlineNoticeTone = "info" | "success" | "warning" | "danger";

export interface InlineNoticeProps
  extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  tone?: InlineNoticeTone;
  icon?: ReactNode;
  title?: ReactNode;
  action?: ReactNode;
}

/** 共享行内通知：启动错误、告警与诊断统一使用同一套填充/文字层级。 */
export function InlineNotice({
  tone = "info",
  icon,
  title,
  action,
  children,
  className,
  ...rest
}: InlineNoticeProps) {
  return (
    <div
      {...rest}
      className={["ni-inline-notice", className].filter(Boolean).join(" ")}
      data-tone={tone}
    >
      {icon != null ? (
        <span className="ni-inline-notice__icon" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <div className="ni-inline-notice__content">
        {title != null ? (
          <strong className="ni-inline-notice__title">{title}</strong>
        ) : null}
        <div className="ni-inline-notice__message">{children}</div>
      </div>
      {action != null ? (
        <div className="ni-inline-notice__action">{action}</div>
      ) : null}
    </div>
  );
}

export default InlineNotice;
