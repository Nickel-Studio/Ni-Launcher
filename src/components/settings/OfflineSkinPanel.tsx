import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Switch } from "../ui";
import type { GameSettings } from "../version/gameSettings";
import { describeError } from "../version/vsettings/api";
import { isTauri } from "../../lib/settings";
import { SettingRow } from "./SettingRow";
import { SettingsGroup } from "./SettingsGroup";

const SKIN_OPTIONS = [
  { value: "random", label: "随机" },
  { value: "steve", label: "Steve" },
  { value: "alex", label: "Alex" },
  { value: "official", label: "正版皮肤" },
  { value: "custom", label: "自定义" },
] as const;

type Props = {
  settings: GameSettings;
  onChange: (value: Partial<GameSettings>) => void;
  disabled: boolean;
};

export function OfflineSkinPanel({ settings, onChange, disabled }: Props) {
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState("");
  const alive = useRef(true);
  const busy = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const mode =
    SKIN_OPTIONS.find((option) => option.value === settings.offlineSkin)
      ?.value ?? "random";
  const name =
    typeof settings.offlineSkinName === "string"
      ? settings.offlineSkinName
      : "";
  const path =
    typeof settings.offlineSkinPath === "string"
      ? settings.offlineSkinPath
      : "";

  async function pickSkin() {
    if (busy.current) return;
    busy.current = true;
    setPicking(true);
    setError("");
    try {
      if (!isTauri()) throw new Error("请在桌面启动器中选择 PNG 皮肤");
      const selected = await invoke<string | null>("game_offline_skin_pick");
      if (selected && alive.current) onChange({ offlineSkinPath: selected });
    } catch (reason) {
      if (alive.current)
        setError(
          `选择皮肤失败：${describeError(reason)}。请重新选择 PNG 文件。`
        );
    } finally {
      busy.current = false;
      if (alive.current) setPicking(false);
    }
  }

  return (
    <SettingsGroup id="game-skin" title="离线皮肤" collapsible>
      <div
        className="st-game__skin-options"
        role="radiogroup"
        aria-label="离线皮肤"
      >
        {SKIN_OPTIONS.map((option) => (
          <label className="st-game__skin-option" key={option.value}>
            <input
              type="radio"
              name="offline-skin"
              value={option.value}
              checked={mode === option.value}
              disabled={picking || (disabled && option.value !== "random")}
              onChange={() => {
                setError("");
                onChange({ offlineSkin: option.value });
              }}
            />
            <span>{option.label}</span>
          </label>
        ))}
      </div>
      {disabled && mode !== "random" ? (
        <p className="st-game__choice-note">
          选择“随机”使用游戏默认皮肤，或刷新运行信息后重试。
        </p>
      ) : null}
      {mode === "official" ? (
        <SettingRow
          label="正版玩家名称"
          fill
          control={
            <input
              className="st-input"
              aria-label="正版玩家名称"
              placeholder="输入正版玩家名称"
              spellCheck={false}
              autoComplete="off"
              value={name}
              disabled={disabled}
              onChange={(event) =>
                onChange({ offlineSkinName: event.target.value })
              }
            />
          }
        />
      ) : null}
      {mode === "custom" ? (
        <>
          <SettingRow
            label="皮肤文件"
            fill
            control={
              <div className="st-game__skin-source">
                <input
                  className="st-input"
                  aria-label="皮肤文件"
                  value={path}
                  placeholder="选择 PNG 皮肤"
                  readOnly
                />
                <Button
                  size="sm"
                  disabled={disabled || picking}
                  onClick={() => void pickSkin()}
                >
                  选择 PNG
                </Button>
                {path ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={picking}
                    onClick={() => {
                      setError("");
                      onChange({ offlineSkinPath: "" });
                    }}
                  >
                    清除皮肤文件
                  </Button>
                ) : null}
              </div>
            }
          />
          <div className="st-game__toggles">
            <Switch
              label="图片使用纤细布局"
              checked={settings.offlineSkinSlim === true}
              disabled={disabled && settings.offlineSkinSlim !== true}
              onChange={(value) => onChange({ offlineSkinSlim: value })}
            />
          </div>
        </>
      ) : null}
      {error ? (
        <p className="st-error" role="alert">
          {error}
        </p>
      ) : null}
    </SettingsGroup>
  );
}
