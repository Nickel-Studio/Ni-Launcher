import {
  Children,
  type CSSProperties,
  type ElementType,
  type ReactNode,
} from "react";

/**
 * Motion primitives — staggered module entrance.
 *
 * `Reveal` renders a single element that fades in and rises 8px on mount.
 * Give it an `index` (and the shared `step`) to stagger a group of siblings:
 *
 *   <Reveal index={0}>…</Reveal>
 *   <Reveal index={1}>…</Reveal>
 *
 * The animation itself lives in `src/styles/ui.css` (`.ni-reveal`) and is fully
 * disabled under `prefers-reduced-motion`.
 */
export interface RevealProps {
  children: ReactNode;
  /** Position within the group; multiplied by `step` to derive the delay. */
  index?: number;
  /** Per-item delay step in ms. */
  step?: number;
  /** Extra base delay in ms. */
  delay?: number;
  /** Element tag to render (default `div`). */
  as?: ElementType;
  className?: string;
  style?: CSSProperties;
}

export function Reveal({
  children,
  index = 0,
  step = 60,
  delay = 0,
  as,
  className,
  style,
}: RevealProps) {
  const Tag = (as ?? "div") as ElementType;
  return (
    <Tag
      className={["ni-reveal", className].filter(Boolean).join(" ")}
      style={{ animationDelay: `${delay + index * step}ms`, ...style }}
    >
      {children}
    </Tag>
  );
}

export interface StaggerProps {
  children: ReactNode;
  /** Per-item delay step in ms. */
  step?: number;
  /** Extra base delay in ms. */
  delay?: number;
  /** Element tag to render (default `div`). */
  as?: ElementType;
  className?: string;
}

/**
 * `Stagger` wraps each direct child in a `Reveal`, assigning the delay by
 * position. The wrapper uses `display: contents` so it does not disturb the
 * parent's layout.
 */
export function Stagger({
  children,
  step = 60,
  delay = 0,
  as,
  className,
}: StaggerProps) {
  const Tag = (as ?? "div") as ElementType;
  const items = Children.toArray(children);
  return (
    <Tag className={["ni-stagger", className].filter(Boolean).join(" ")}>
      {items.map((child, i) => (
        <div
          key={i}
          className="ni-reveal"
          style={{ animationDelay: `${delay + i * step}ms` }}
        >
          {child}
        </div>
      ))}
    </Tag>
  );
}

export default Reveal;
