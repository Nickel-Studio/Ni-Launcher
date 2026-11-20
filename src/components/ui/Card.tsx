import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

export type CardPadding = "none" | "sm" | "md" | "lg";

export interface CardProps
  extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  padding?: CardPadding;
  interactive?: boolean;
  glass?: boolean;
  title?: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
}

export const Card = forwardRef<HTMLDivElement, CardProps>(function Card(
  {
    padding = "md",
    interactive = false,
    glass = false,
    title,
    subtitle,
    actions,
    className,
    children,
    ...rest
  },
  ref
) {
  const cls = [
    "ni-card",
    `ni-card--pad-${padding}`,
    interactive && "ni-card--interactive",
    glass && "ni-card--glass",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const hasHead = title != null || subtitle != null || actions != null;

  return (
    <div ref={ref} className={cls} {...rest}>
      {hasHead ? (
        <div className="ni-card__head">
          <div className="ni-card__titles">
            {title != null ? <h3 className="ni-card__title">{title}</h3> : null}
            {subtitle != null ? (
              <div className="ni-card__subtitle">{subtitle}</div>
            ) : null}
          </div>
          {actions != null ? (
            <div className="ni-card__actions">{actions}</div>
          ) : null}
        </div>
      ) : null}
      {children}
    </div>
  );
});
