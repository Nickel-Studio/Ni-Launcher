import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { ICONS } from "./icons";
import { Tooltip } from "./Tooltip";

export type ButtonVariant =
  | "primary"
  | "secondary"
  | "outline"
  | "ghost"
  | "danger";
export type ButtonSize = "sm" | "md" | "lg";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  fullWidth?: boolean;
  /** Alias of `fullWidth`. */
  block?: boolean;
  /** Optional named leading icon — a key from the icon registry, e.g. "play". */
  icon?: string;
  iconLeft?: ReactNode;
  iconRight?: ReactNode;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  function Button(
    {
      variant = "secondary",
      size = "md",
      loading = false,
      fullWidth = false,
      block = false,
      icon,
      iconLeft,
      iconRight,
      className,
      children,
      disabled,
      title,
      type = "button",
      ...rest
    },
    ref
  ) {
    const NamedIcon = icon ? ICONS[icon] : undefined;

    const cls = [
      "nw-btn",
      `nw-btn--${variant}`,
      size !== "md" && `nw-btn--${size}`,
      fullWidth || block ? "nw-btn--full" : false,
      className,
    ]
      .filter(Boolean)
      .join(" ");

    const leading = loading ? (
      <span className="nw-btn__spinner" aria-hidden />
    ) : iconLeft != null ? (
      iconLeft
    ) : NamedIcon ? (
      <NamedIcon />
    ) : null;

    const control = (
      <button
        ref={ref}
        type={type}
        className={cls}
        disabled={disabled || loading}
        {...rest}
      >
        {leading}
        {children != null ? <span>{children}</span> : null}
        {iconRight}
      </button>
    );
    // 克隆原按钮，不新增包裹元素，保留布局、ref 与可访问名称。
    return title ? <Tooltip label={title}>{control}</Tooltip> : control;
  }
);
