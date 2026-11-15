import { IconPlay } from "../account/icons";
import "./launch.css";

export interface LaunchButtonProps {
  running: boolean;
  disabled?: boolean;
  onClick: () => void;
  /** 主行文案 */
  label?: string;
  /** 副行文案：当前选中版本名 */
  subLabel?: string;
}

/**
 * 大号启动按钮（去线框化：accent 实心大圆角块，无描边）。
 * 主行「启动游戏」+ 副行当前版本名。
 */
export function LaunchButton({
  running,
  disabled = false,
  onClick,
  label = "启动游戏",
  subLabel,
}: LaunchButtonProps) {
  return (
    <button
      type="button"
      className="launch-button"
      data-running={running}
      disabled={disabled || running}
      onClick={onClick}
    >
      <span className="launch-button__icon" aria-hidden="true">
        {running ? <span className="launch-button__spinner" /> : <IconPlay />}
      </span>
      <span className="launch-button__text">
        <span className="launch-button__main">{label}</span>
        {subLabel ? (
          <span className="launch-button__sub">{subLabel}</span>
        ) : null}
      </span>
    </button>
  );
}

export default LaunchButton;
