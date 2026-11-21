import type { KeyboardEvent, ReactNode } from "react";
import { useSlidePill } from "../../lib/useSlidePill";

export interface SegmentOption {
  value: string;
  label: ReactNode;
  icon?: ReactNode;
}

export interface SegmentedControlProps {
  value: string;
  onChange: (value: string) => void;
  options: SegmentOption[];
  size?: "sm" | "md";
  fullWidth?: boolean;
  className?: string;
}

export function SegmentedControl({
  value,
  onChange,
  options,
  size = "md",
  fullWidth = false,
  className,
}: SegmentedControlProps) {
  const cls = [
    "ni-seg",
    size === "sm" && "ni-seg--sm",
    fullWidth && "ni-seg--full",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  const activeIndex = options.findIndex((option) => option.value === value);
  const { navRef, registerTab, pill } = useSlidePill<
    HTMLDivElement,
    HTMLButtonElement
  >(activeIndex);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || event.nativeEvent.isComposing) return;
    const keys = ["ArrowLeft", "ArrowRight", "Home", "End"];
    if (!keys.includes(event.key) || options.length === 0) return;

    event.preventDefault();
    const current = activeIndex >= 0 ? activeIndex : 0;
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
        ? options.length - 1
        : event.key === "ArrowRight"
        ? (current + 1) % options.length
        : (current - 1 + options.length) % options.length;

    onChange(options[next].value);
    const tabs =
      navRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[next]?.focus({ preventScroll: true });
  };

  return (
    <div
      ref={navRef}
      className={cls}
      role="tablist"
      aria-orientation="horizontal"
      onKeyDown={handleKeyDown}
    >
      <span
        className="ni-seg__pill"
        data-ready={pill.ready}
        style={{ transform: `translateX(${pill.left}px)`, width: pill.width }}
        aria-hidden="true"
      />
      {options.map((opt, index) => (
        <button
          key={opt.value}
          ref={registerTab(index)}
          type="button"
          role="tab"
          aria-selected={opt.value === value}
          data-active={opt.value === value}
          className="ni-seg__item"
          onClick={() => onChange(opt.value)}
        >
          {opt.icon}
          {typeof opt.label === "string" ? <span>{opt.label}</span> : opt.label}
        </button>
      ))}
    </div>
  );
}
