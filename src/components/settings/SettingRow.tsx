import type { ReactNode } from "react";
import { Tooltip } from "../ui";

type Props = {
  label: ReactNode;
  description?: ReactNode;
  control: ReactNode;
  /** 提示只绑定控件容器；标签不增加焦点与鼠标形态。 */
  help?: string;
  /** 仅为图片、色板等大块内容使用上下布局。 */
  stacked?: boolean;
  /** 标签与宽控件同排，控件占据剩余空间。 */
  fill?: boolean;
};

export function SettingRow({
  label,
  description,
  control,
  help,
  stacked,
  fill,
}: Props) {
  const classes = [
    "st-row",
    stacked && "st-row--stacked",
    fill && "st-row--fill",
  ]
    .filter(Boolean)
    .join(" ");
  const controlNode = <div className="st-row__control">{control}</div>;
  return (
    <div className={classes}>
      <div className="st-row__text">
        <div className="st-row__label">{label}</div>
        {description && <div className="st-row__desc">{description}</div>}
      </div>
      {help ? <Tooltip label={help}>{controlNode}</Tooltip> : controlNode}
    </div>
  );
}
