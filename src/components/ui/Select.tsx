import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { IconCheck, IconChevronDown } from "./icons";

export interface SelectOption {
  value: string;
  label: ReactNode;
  disabled?: boolean;
  /** Optional group heading; consecutive options sharing a group render together. */
  group?: string;
}

export interface SelectProps {
  value: string;
  onChange: (value: string) => void;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  size?: "sm" | "md";
  className?: string;
  id?: string;
  ariaLabel?: string;
}

export function Select({
  value,
  onChange,
  options,
  placeholder = "请选择",
  disabled = false,
  size = "md",
  className,
  id,
  ariaLabel,
}: SelectProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDocMouseDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node))
        setOpen(false);
    };
    document.addEventListener("mousedown", onDocMouseDown);
    return () => document.removeEventListener("mousedown", onDocMouseDown);
  }, [open]);

  const selected = options.find((o) => o.value === value);
  const cls = ["ni-select", size === "sm" && "ni-select--sm", className]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={cls}
      ref={rootRef}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
      onKeyDown={(event) => {
        if (
          !open ||
          event.key !== "Escape" ||
          event.defaultPrevented ||
          event.nativeEvent.isComposing
        )
          return;
        // 当前列表先消费 Escape，不连带关闭外层弹窗。
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        triggerRef.current?.focus({ preventScroll: true });
      }}
    >
      <button
        ref={triggerRef}
        id={id}
        type="button"
        className="ni-select__trigger"
        data-open={open}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="ni-select__value" data-placeholder={!selected}>
          {selected ? selected.label : placeholder}
        </span>
        <IconChevronDown className="ni-select__chevron" />
      </button>

      {open ? (
        <div className="ni-select__menu" role="listbox">
          {options.map((opt, index) => {
            const prevGroup = index > 0 ? options[index - 1].group : undefined;
            const showGroup = opt.group != null && opt.group !== prevGroup;
            return (
              <Fragment key={opt.value}>
                {showGroup ? (
                  <div className="ni-select__group">{opt.group}</div>
                ) : null}
                <button
                  type="button"
                  role="option"
                  aria-selected={opt.value === value}
                  data-selected={opt.value === value}
                  disabled={opt.disabled}
                  className="ni-select__option"
                  onClick={() => {
                    if (opt.disabled) return;
                    onChange(opt.value);
                    setOpen(false);
                    triggerRef.current?.focus({ preventScroll: true });
                  }}
                >
                  <span>{opt.label}</span>
                  {opt.value === value ? <IconCheck /> : null}
                </button>
              </Fragment>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
