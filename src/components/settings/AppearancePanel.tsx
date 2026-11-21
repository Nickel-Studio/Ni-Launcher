import { useRef, useState } from "react";
import type { ChangeEvent, CSSProperties } from "react";
import { Button, SegmentedControl, Slider, Tooltip } from "../ui";
import { useBackgroundSrc } from "../../lib/settings";
import { FORTY_TWO_END, hslToRgb, rgbToHex } from "../../lib/theme";
import { ACCENT_OPTIONS, THEME_OPTIONS } from "./accents";
import { pickImagePath, readFileAsDataUrl } from "./backgroundImage";
import { SettingRow } from "./SettingRow";
import { SettingsGroup } from "./SettingsGroup";
import { ThemePreview } from "./ThemePreview";
import type { AppSettings, ThemeMode } from "./types";

type Props = {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
  onOpenCustom?: () => void;
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

/** Appearance owns background controls; custom color controls have their own page. */
export function AppearancePanel({ settings, update, onOpenCustom }: Props) {
  const fileRef = useRef<HTMLInputElement | null>(null);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState("");
  const backgroundUrl = useBackgroundSrc(settings.backgroundImage);
  const hasBackground = Boolean(backgroundUrl);
  const selectedAccent = ACCENT_OPTIONS.find(
    (option) => option.value === settings.accent
  );
  const customColor = rgbToHex(
    hslToRgb(
      settings.customTheme.hue,
      settings.customTheme.saturation,
      settings.customTheme.lightness
    )
  );

  async function handleChoose() {
    if (picking) return;
    setError("");
    setPicking(true);
    try {
      const path = await pickImagePath();
      if (path) update({ backgroundImage: path });
    } catch {
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
        description="保留中性阅读区域；新主题还带有原创背景与顶栏装饰"
        control={
          <div className="st-accents" role="radiogroup" aria-label="颜色主题">
            {ACCENT_OPTIONS.map((a, index) => {
              const active = settings.accent === a.value;
              return (
                <Tooltip
                  key={a.value}
                  label={
                    a.description ? `${a.label} · ${a.description}` : a.label
                  }
                >
                  <button
                    type="button"
                    role="radio"
                    aria-label={a.label}
                    aria-checked={active}
                    tabIndex={active ? 0 : -1}
                    className={`st-swatch st-swatch--${a.value}${
                      active ? " is-active" : ""
                    }`}
                    style={
                      {
                        "--swatch":
                          a.value === "custom" ? customColor : a.color,
                        "--swatch-end": FORTY_TWO_END,
                      } as CSSProperties
                    }
                    onClick={() => update({ accent: a.value })}
                    onKeyDown={(event) => {
                      const step =
                        event.key === "ArrowRight" || event.key === "ArrowDown"
                          ? 1
                          : event.key === "ArrowLeft" || event.key === "ArrowUp"
                          ? -1
                          : 0;
                      if (!step && event.key !== "Home" && event.key !== "End")
                        return;
                      event.preventDefault();
                      const next =
                        event.key === "Home"
                          ? 0
                          : event.key === "End"
                          ? ACCENT_OPTIONS.length - 1
                          : (index + step + ACCENT_OPTIONS.length) %
                            ACCENT_OPTIONS.length;
                      update({ accent: ACCENT_OPTIONS[next].value });
                      event.currentTarget
                        .closest(".st-accents")
                        ?.querySelectorAll<HTMLButtonElement>('[role="radio"]')
                        [next]?.focus();
                    }}
                  >
                    <span className="st-swatch__dot" aria-hidden="true" />
                    <span className="st-swatch__name">{a.label}</span>
                  </button>
                </Tooltip>
              );
            })}
          </div>
        }
      />
      {selectedAccent?.description ? (
        <div className="st-theme-note">
          <p>
            {selectedAccent.description}
            {settings.accent === "fortyTwo"
              ? "。窗口隐藏时暂停，减弱动效时使用静态青绿。"
              : "。"}
          </p>
          {settings.accent === "custom" && onOpenCustom ? (
            <Button variant="secondary" size="sm" onClick={onOpenCustom}>
              调整自定义主题
            </Button>
          ) : null}
        </div>
      ) : null}
      <SettingRow
        stacked
        label="背景图片"
        description="选择本地图片后优先显示用户背景，不叠加主题背景装饰"
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
      {error ? (
        <p className="st-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="st-preview-wrap">
        <div className="st-preview-wrap__hint ni-eyebrow">实时预览</div>
        <ThemePreview />
      </div>
    </SettingsGroup>
  );
}
