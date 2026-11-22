import { useRef, useState } from "react";
import type { ChangeEvent, CSSProperties } from "react";
import { Button, Collapse, SegmentedControl, Slider, Tooltip } from "../ui";
import { isTauri } from "../../lib/settings";
import { BackgroundMedia } from "../ui/BackgroundMedia";
import { FORTY_TWO_END, hslToRgb, rgbToHex } from "../../lib/theme";
import { ACCENT_OPTIONS, THEME_OPTIONS } from "./accents";
import { BACKGROUND_ACCEPT, pickBackgroundPath, readFileAsDataUrl } from "./backgroundImage";
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
  const backgroundPath = settings.backgroundImage?.trim() || null;
  const hasBackground = Boolean(backgroundPath);
  const [failedPath, setFailedPath] = useState<string | null>(null);
  const customSelected = settings.accent === "custom";
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
      if (!isTauri()) {
        fileRef.current?.click();
        return;
      }
      const path = await pickBackgroundPath();
      if (path) {
        setFailedPath(null);
        update({ backgroundImage: path });
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPicking(false);
    }
  }

  async function handleFile(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    setError("");
    setPicking(true);
    try {
      const dataUrl = await readFileAsDataUrl(file);
      setFailedPath(null);
      update({ backgroundImage: dataUrl });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "无法读取该文件，请更换后重试。");
    } finally {
      setPicking(false);
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
        description="保留中性阅读区域；特殊主题带有背景与顶部装饰"
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
      <Collapse open={customSelected} staggered className="st-custom-note">
        <div className="st-theme-note">
          <p>按自己的喜好调整颜色与顶部装饰。</p>
          {onOpenCustom ? (
            <Button variant="secondary" size="sm" onClick={onOpenCustom}>
              调整自定义主题
            </Button>
          ) : null}
        </div>
      </Collapse>
      <SettingRow
        stacked
        label="背景图片 / 视频"
        description="支持本地图片、MP4、WebM 和 OGV；视频静音循环播放，不叠加主题背景装饰"
        control={
          <div className="st-bg">
            <div
              className="st-bg__thumb"
              data-empty={hasBackground ? "false" : "true"}
            >
              {backgroundPath ? (
                <>
                  <BackgroundMedia
                    key={backgroundPath}
                    path={backgroundPath}
                    animate={false}
                    onLoad={() => setFailedPath(null)}
                    onError={() => setFailedPath(backgroundPath)}
                  />
                  {failedPath === backgroundPath ? (
                    <span className="st-bg__thumb-error">背景无法加载，请更换文件</span>
                  ) : null}
                </>
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
                {hasBackground ? "更换背景" : "选择图片或视频"}
              </Button>
              <Button
                variant="secondary"
                onClick={() => {
                  setFailedPath(null);
                  setError("");
                  update({ backgroundImage: null });
                }}
                disabled={!hasBackground}
              >
                清除背景
              </Button>
            </div>
            <input
              ref={fileRef}
              type="file"
              accept={BACKGROUND_ACCEPT}
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
