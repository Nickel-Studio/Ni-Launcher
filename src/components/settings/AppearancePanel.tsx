import { useRef, useState } from "react";
import type { ChangeEvent, CSSProperties } from "react";
import { Button, SegmentedControl, Slider, Tooltip } from "../ui";
import { useBackgroundSrc } from "../../lib/settings";
import { ACCENT_OPTIONS, THEME_OPTIONS } from "./accents";
import { pickImagePath, readFileAsDataUrl } from "./backgroundImage";
import { SettingRow } from "./SettingRow";
import { SettingsGroup } from "./SettingsGroup";
import { ThemePreview } from "./ThemePreview";
import type { AccentColor, AppSettings, ThemeMode } from "./types";

type Props = {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
};

const THEME_SEGMENTS = THEME_OPTIONS.map((o) => ({
  value: o.value,
  label: o.label,
}));
const BLUR_LABELS = ["关", "弱", "中", "强", "极强"];

function blurLabel(value: number): string {
  if (value <= 0) return BLUR_LABELS[0];
  const idx = Math.min(
    BLUR_LABELS.length - 1,
    Math.ceil((value / 40) * (BLUR_LABELS.length - 1))
  );
  return BLUR_LABELS[idx];
}

/** A. 外观：明暗 / 颜色主题 / 背景图 / 模糊 / 遮罩 / 实时预览。 */
export function AppearancePanel({ settings, update }: Props) {
  const fileRef = useRef<HTMLInputElement | null>(null);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState("");

  const backgroundUrl = useBackgroundSrc(settings.backgroundImage);
  const hasBackground = Boolean(backgroundUrl);

  async function handleChoose() {
    if (picking) return;
    setError("");
    setPicking(true);
    try {
      const path = await pickImagePath();
      if (path) update({ backgroundImage: path });
    } catch {
      // 插件不可用或不在 Tauri 环境：回退到原生文件选择
      fileRef.current?.click();
    } finally {
      setPicking(false);
    }
  }

  async function handleFile(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    try {
      const dataUrl = await readFileAsDataUrl(file);
      update({ backgroundImage: dataUrl });
    } catch {
      setError("无法读取该图片，请更换文件后重试。");
    }
  }

  return (
    <SettingsGroup
      id="appearance"
      title="外观"
      description="明暗模式、颜色主题与背景效果，修改后即时生效"
    >
      <SettingRow
        label="明暗模式"
        description="跟随系统，或手动指定亮色 / 暗色"
        control={
          <SegmentedControl
            value={settings.themeMode}
            onChange={(v: string) => update({ themeMode: v as ThemeMode })}
            options={THEME_SEGMENTS}
          />
        }
      />

      <SettingRow
        stacked
        label="颜色主题"
        description="选择颜色主题，应用于按钮、选中态与高亮元素"
        control={
          <div className="st-accents" role="radiogroup" aria-label="颜色主题">
            {ACCENT_OPTIONS.map((a) => {
              const active = settings.accent === a.value;
              // mono（黑与白）的圆点是黑白分割的双色块，需要额外的修饰类。
              const mono = a.value === "mono";
              return (
                <Tooltip key={a.value} label={a.label}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={active}
                    className={`st-swatch${mono ? " st-swatch--mono" : ""}${
                      active ? " is-active" : ""
                    }`}
                    style={{ "--swatch": a.color } as CSSProperties}
                    onClick={() => update({ accent: a.value as AccentColor })}
                  >
                    <span className="st-swatch__dot" />
                    <span className="st-swatch__name">{a.label}</span>
                  </button>
                </Tooltip>
              );
            })}
          </div>
        }
      />

      <SettingRow
        stacked
        label="背景图片"
        description="选择一张本地图片作为启动器背景"
        control={
          <div className="st-bg">
            <div
              className="st-bg__thumb"
              data-empty={hasBackground ? "false" : "true"}
            >
              {backgroundUrl ? (
                <img src={backgroundUrl} alt="背景预览" />
              ) : (
                <span className="st-bg__thumb-empty">未设置背景</span>
              )}
            </div>
            <div className="st-bg__actions">
              <Button
                variant="primary"
                loading={picking}
                onClick={handleChoose}
              >
                {hasBackground ? "更换图片" : "选择图片"}
              </Button>
              <Button
                variant="secondary"
                onClick={() => update({ backgroundImage: null })}
                disabled={!hasBackground}
              >
                清除背景
              </Button>
            </div>
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              hidden
              onChange={handleFile}
            />
          </div>
        }
      />

      <SettingRow
        fill
        label="背景模糊度"
        description={`当前强度：${blurLabel(settings.backgroundBlur)}`}
        control={
          <Slider
            min={0}
            max={40}
            step={1}
            suffix="px"
            value={settings.backgroundBlur}
            aria-label="背景模糊度"
            onChange={(v: number) => update({ backgroundBlur: Math.round(v) })}
          />
        }
      />

      <SettingRow
        fill
        label="背景遮罩浓度"
        description="调高以突出内容"
        control={
          <Slider
            min={0}
            max={100}
            step={1}
            suffix="%"
            value={Math.round(settings.backgroundOpacity * 100)}
            aria-label="背景遮罩浓度"
            onChange={(v: number) =>
              update({ backgroundOpacity: Number((v / 100).toFixed(2)) })
            }
          />
        }
      />

      {error && (
        <p className="st-error" role="alert">
          {error}
        </p>
      )}

      <div className="st-preview-wrap">
        <div className="st-preview-wrap__hint nw-eyebrow">实时预览</div>
        <ThemePreview />
      </div>
    </SettingsGroup>
  );
}
