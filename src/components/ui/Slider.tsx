import type { CSSProperties } from "react";

export interface SliderProps {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  suffix?: string;
  formatValue?: (value: number) => string;
  showValue?: boolean;
  disabled?: boolean;
  className?: string;
  id?: string;
  "aria-label"?: string;
}

export function Slider({
  value,
  onChange,
  min = 0,
  max = 100,
  step = 1,
  suffix = "",
  formatValue,
  showValue = true,
  disabled = false,
  className,
  id,
  "aria-label": ariaLabel,
}: SliderProps) {
  const bounded = Math.min(max, Math.max(min, value));
  const pct = max > min ? ((bounded - min) / (max - min)) * 100 : 0;
  const shown = formatValue
    ? formatValue(bounded)
    : `${Number.isInteger(bounded) ? bounded : bounded.toFixed(2)}${suffix}`;
  const style = { "--_p": `${pct}%` } as CSSProperties;
  return (
    <div className={["nw-slider-row", className].filter(Boolean).join(" ")}>
      <input
        id={id}
        type="range"
        className="nw-slider"
        min={min}
        max={max}
        step={step}
        value={bounded}
        disabled={disabled}
        style={style}
        aria-label={ariaLabel}
        aria-valuetext={shown}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      {showValue ? <span className="nw-slider-row__val">{shown}</span> : null}
    </div>
  );
}
