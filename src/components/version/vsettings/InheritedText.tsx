import type { ChangeEvent } from "react";
import { Button, Select } from "../../ui";

interface InheritedTextProps {
  label: string;
  value: string | null;
  globalValue: string;
  onChange: (value: string | null) => void;
  multiline?: boolean;
  placeholder?: string;
}

/**
 * 实例独立设置的可继承文本。
 *
 * 持久化契约保持不变：`null` = 跟随全局，字符串（含空串）= 单独设置。
 * 视觉上收敛为「输入框为主 + 右侧紧凑切换 + 按需清空」：
 * - 跟随全局时输入框只读展示全局值，切换按钮显示「跟随全局」；
 * - 单独设置时输入框可编辑，切换按钮显示「单独设置」；
 * - 「清空」只在单独设置且当前值非空时出现，避免每个字段都占一行三件套。
 */
export function InheritedText({
  label,
  value,
  globalValue,
  onChange,
  multiline = false,
  placeholder,
}: InheritedTextProps) {
  const inherited = value === null;
  const inputProps = {
    "aria-label": label,
    value: inherited ? globalValue : value,
    disabled: inherited,
    placeholder: inherited ? "跟随全局设置" : placeholder,
    spellCheck: false,
    onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
      onChange(event.target.value),
  };

  return (
    <div className={`vst-inherit${multiline ? " vst-inherit--multiline" : ""}`}>
      <div className="vst-inherit__field">
        {multiline ? (
          <textarea
            {...inputProps}
            className="vst-input vst-textarea"
            rows={3}
          />
        ) : (
          <input {...inputProps} className="vst-input" />
        )}
        <Button
          size="sm"
          variant="ghost"
          className="vst-inherit__toggle"
          data-inherited={inherited ? "true" : "false"}
          aria-pressed={inherited}
          aria-label={`${label}：${
            inherited
              ? "当前跟随全局，点击改为单独设置"
              : "当前单独设置，点击恢复跟随全局"
          }`}
          onClick={() => onChange(inherited ? globalValue : null)}
        >
          {inherited ? "跟随全局" : "单独设置"}
        </Button>
      </div>
      {!inherited && value !== "" ? (
        <Button
          size="sm"
          variant="ghost"
          className="vst-inherit__clear"
          onClick={() => onChange("")}
        >
          清空
        </Button>
      ) : null}
    </div>
  );
}

interface InheritedBooleanProps {
  label: string;
  value: boolean | null;
  globalValue: boolean;
  onChange: (value: boolean | null) => void;
  disabled?: boolean;
}

/**
 * 三态布尔设置：选择控件为主，继承状态用紧凑标签补充。
 * 仍保留 `null`（跟随全局）/ `true` / `false` 三种持久化值。
 */
export function InheritedBoolean({
  label,
  value,
  globalValue,
  onChange,
  disabled = false,
}: InheritedBooleanProps) {
  return (
    <div className="vst-inherit-boolean">
      <Select
        ariaLabel={`${label}（全局：${globalValue ? "开启" : "关闭"}）`}
        value={value === null ? "global" : String(value)}
        disabled={disabled}
        options={[
          { value: "global", label: "跟随全局" },
          { value: "true", label: "开启" },
          { value: "false", label: "关闭" },
        ]}
        onChange={(next) =>
          onChange(next === "global" ? null : next === "true")
        }
      />
      {value === null ? (
        <span className="vst-inherit__state" aria-hidden>
          全局：{globalValue ? "开启" : "关闭"}
        </span>
      ) : null}
    </div>
  );
}
