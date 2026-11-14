import type { ReactNode } from "react";

export interface TabItem {
  value: string;
  label: ReactNode;
  icon?: ReactNode;
  badge?: ReactNode;
}

export interface TabsProps {
  value: string;
  onChange: (value: string) => void;
  items: TabItem[];
  variant?: "underline" | "pills";
  className?: string;
}

export function Tabs({ value, onChange, items, variant = "underline", className }: TabsProps) {
  const cls = ["nw-tabs", `nw-tabs--${variant}`, className].filter(Boolean).join(" ");

  return (
    <div className={cls} role="tablist">
      {items.map((item) => (
        <button
          key={item.value}
          type="button"
          role="tab"
          aria-selected={item.value === value}
          data-active={item.value === value}
          className="nw-tab"
          onClick={() => onChange(item.value)}
        >
          {item.icon}
          <span>{item.label}</span>
          {item.badge != null ? <span className="nw-tab__badge">{item.badge}</span> : null}
        </button>
      ))}
    </div>
  );
}
