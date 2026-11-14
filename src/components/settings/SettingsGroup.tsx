import { useState, type ReactNode } from "react";
import { Card, Collapse, IconChevronDown } from "../ui";

type Props = {
  id: string;
  title: string;
  description?: string;
  children: ReactNode;
  collapsible?: boolean;
};

/** 分组卡片复用 Card；高级设置可原位收起并退出键盘序列。 */
export function SettingsGroup({
  id,
  title,
  description,
  children,
  collapsible = false,
}: Props) {
  const [open, setOpen] = useState(true);
  if (collapsible)
    return (
      <Card
        id={id}
        className="st-group st-group--collapsible"
        padding="md"
        role="region"
        aria-label={title}
      >
        <h3 className="st-group__heading">
          <button
            className="st-group__toggle"
            type="button"
            aria-expanded={open}
            aria-controls={`${id}-body`}
            onClick={() => setOpen((value) => !value)}
          >
            <span>{title}</span>
            <IconChevronDown />
          </button>
        </h3>
        <Collapse open={open} id={`${id}-body`}>
          <div className="st-group__body st-group__body--collapsible">
            {description ? (
              <p className="st-group__description">{description}</p>
            ) : null}
            {children}
          </div>
        </Collapse>
      </Card>
    );
  return (
    <Card
      id={id}
      className="st-group"
      padding="md"
      role="region"
      aria-label={title}
      title={title}
      subtitle={description}
    >
      <div className="st-group__body">{children}</div>
    </Card>
  );
}
