import {
  forwardRef,
  useId,
  type ButtonHTMLAttributes,
  type ReactNode,
} from "react";

export interface SwitchProps
  extends Omit<
    ButtonHTMLAttributes<HTMLButtonElement>,
    "onChange" | "children"
  > {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label?: ReactNode;
  description?: ReactNode;
}

/** 按钮属性和 ref 透传到开关本体，让 Tooltip 与键盘名称使用同一触发元素。 */
export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(
  function Switch(
    { checked, onChange, label, description, className, onClick, ...rest },
    ref
  ) {
    const labelId = useId();
    const descriptionId = useId();
    const hasField = label != null || description != null;
    const control = (
      <button
        {...rest}
        ref={ref}
        type="button"
        role="switch"
        aria-checked={checked}
        aria-labelledby={
          rest["aria-labelledby"] ?? (label != null ? labelId : undefined)
        }
        aria-describedby={
          [
            rest["aria-describedby"],
            description != null ? descriptionId : undefined,
          ]
            .filter(Boolean)
            .join(" ") || undefined
        }
        data-on={checked}
        className={["ni-switch", !hasField && className]
          .filter(Boolean)
          .join(" ")}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) onChange(!checked);
        }}
      >
        <span className="ni-switch__knob" />
      </button>
    );
    if (!hasField) return control;
    return (
      <div className={["ni-field", className].filter(Boolean).join(" ")}>
        <span className="ni-field__label">
          {label != null ? <strong id={labelId}>{label}</strong> : null}
          {description != null ? (
            <span id={descriptionId}>{description}</span>
          ) : null}
        </span>
        {control}
      </div>
    );
  }
);
