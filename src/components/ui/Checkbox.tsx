import {
  forwardRef,
  type ButtonHTMLAttributes,
  type ReactNode,
} from "react";
import { IconCheck } from "./icons";

export interface CheckboxProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange" | "children"> {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** 勾选框右侧文案；按钮整体是同一个可点击控件。 */
  label?: ReactNode;
}

/** 按钮承载 checkbox 语义，方框与文案共用一个键盘焦点。 */
export const Checkbox = forwardRef<HTMLButtonElement, CheckboxProps>(
  function Checkbox(
    { checked, onChange, label, className, onClick, disabled, ...rest },
    ref
  ) {
    return (
      <button
        {...rest}
        ref={ref}
        type="button"
        role="checkbox"
        aria-checked={checked}
        disabled={disabled}
        data-checked={checked}
        className={["nw-check", className].filter(Boolean).join(" ")}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) onChange(!checked);
        }}
      >
        <span className="nw-check__box" aria-hidden="true">
          {checked ? <IconCheck /> : null}
        </span>
        {label != null ? <span className="nw-check__label">{label}</span> : null}
      </button>
    );
  }
);
