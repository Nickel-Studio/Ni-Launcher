import { IconPlay } from "../account/icons";
import { IconClose } from "../ui/icons";
import "./launch.css";

export interface LaunchButtonProps {
  starting: boolean;
  cancelRequested: boolean;
  disabled?: boolean;
  onClick: () => void;
  onCancel: () => void;
  subLabel?: string;
}

const BUTTON_STATES = {
  idle: { label: "启动游戏", icon: <IconPlay /> },
  cancel: { label: "取消启动", icon: <IconClose /> },
  cancelling: {
    label: "正在取消...",
    icon: <span className="launch-button__spinner" />,
  },
};

export function LaunchButton({
  starting,
  cancelRequested,
  disabled = false,
  onClick,
  onCancel,
  subLabel,
}: LaunchButtonProps) {
  const state = starting ? (cancelRequested ? "cancelling" : "cancel") : "idle";

  return (
    <button
      type="button"
      className="launch-button"
      data-state={state}
      disabled={starting ? cancelRequested : disabled}
      onClick={starting ? onCancel : onClick}
      aria-label={BUTTON_STATES[state].label}
      aria-busy={cancelRequested}
      title={subLabel}
    >
      <span className="launch-button__icon" aria-hidden="true">
        {Object.entries(BUTTON_STATES).map(([key, value]) => (
          <span
            key={key}
            className="launch-button__glyph"
            data-visible={state === key}
          >
            {value.icon}
          </span>
        ))}
      </span>
      <span className="launch-button__text" aria-hidden="true">
        <span className="launch-button__main">
          {Object.entries(BUTTON_STATES).map(([key, value]) => (
            <span
              key={key}
              className="launch-button__label"
              data-visible={state === key}
            >
              {value.label}
            </span>
          ))}
        </span>
        {subLabel ? (
          <span className="launch-button__sub">{subLabel}</span>
        ) : null}
      </span>
    </button>
  );
}

export default LaunchButton;
