import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Tooltip } from "./Tooltip";

export type IconButtonVariant = "ghost" | "solid" | "outline";
export type IconButtonSize = "sm" | "md" | "lg";

export interface IconButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Required accessible name（作为 aria-label，同时作为自绘 Tooltip 的文案）。 */
  label: string;
  variant?: IconButtonVariant;
  size?: IconButtonSize;
  active?: boolean;
  children: ReactNode;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(
  function IconButton(
    {
      label,
      title,
      variant = "ghost",
      size = "md",
      active = false,
      className,
      children,
      type = "button",
      ...rest
    },
    ref
  ) {
    const cls = [
      "nw-iconbtn",
      `nw-iconbtn--${variant}`,
      size !== "md" && `nw-iconbtn--${size}`,
      className,
    ]
      .filter(Boolean)
      .join(" ");

    return (
      <Tooltip label={label || title || ""}>
        <button
          ref={ref}
          type={type}
          className={cls}
          data-active={active}
          aria-label={label}
          aria-pressed={active || undefined}
          {...rest}
        >
          {children}
        </button>
      </Tooltip>
    );
  }
);
