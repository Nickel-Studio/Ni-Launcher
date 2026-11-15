import { Button, Slider } from "../ui";
import {
  CUSTOM_THEME_BOUNDS,
  DEFAULT_CUSTOM_THEME,
  normalizeCustomTheme,
  THEME_DECORATIONS,
} from "../../lib/theme";
import { ThemeArtwork } from "../ui/ThemeArtwork";
import { SettingRow } from "./SettingRow";
import { SettingsGroup } from "./SettingsGroup";
import { ThemePreview } from "./ThemePreview";
import type { AppSettings, CustomTheme, ThemeDecoration } from "./types";

type Props = {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
};

const DECORATION_LABELS: Record<ThemeDecoration, string> = {
  none: "无装饰",
  lattice: "晶格",
  ribbons: "流带",
  orbits: "环轨",
  folds: "折纸",
};

export function CustomThemePanel({ settings, update }: Props) {
  const custom = settings.customTheme;
  const change = (patch: Partial<CustomTheme>) =>
    update({ customTheme: normalizeCustomTheme({ ...custom, ...patch }) });
  return (
    <SettingsGroup id="custom-theme" title="自定义主题">
      <SettingRow
        fill
        label="色相"
        control={
          <Slider
            min={CUSTOM_THEME_BOUNDS.hue[0]}
            max={CUSTOM_THEME_BOUNDS.hue[1]}
            step={1}
            suffix="°"
            value={custom.hue}
            aria-label="主题色相"
            onChange={(hue) => change({ hue })}
          />
        }
      />
      <SettingRow
        fill
        label="饱和度"
        control={
          <Slider
            min={CUSTOM_THEME_BOUNDS.saturation[0]}
            max={CUSTOM_THEME_BOUNDS.saturation[1]}
            step={1}
            suffix="%"
            value={custom.saturation}
            aria-label="主题饱和度"
            onChange={(saturation) => change({ saturation })}
          />
        }
      />
      <SettingRow
        fill
        label="明度"
        control={
          <Slider
            min={CUSTOM_THEME_BOUNDS.lightness[0]}
            max={CUSTOM_THEME_BOUNDS.lightness[1]}
            step={1}
            suffix="%"
            value={custom.lightness}
            aria-label="主题明度"
            onChange={(lightness) => change({ lightness })}
          />
        }
      />
      <SettingRow
        fill
        label="顶栏色相偏移"
        description={
          settings.titlebarTint
            ? undefined
            : "需在更多 → 前沿计划打开「顶栏颜色小改」"
        }
        control={
          <Slider
            min={CUSTOM_THEME_BOUNDS.titlebarHueShift[0]}
            max={CUSTOM_THEME_BOUNDS.titlebarHueShift[1]}
            step={1}
            suffix="°"
            value={custom.titlebarHueShift}
            disabled={!settings.titlebarTint}
            aria-label="顶栏色相偏移"
            onChange={(titlebarHueShift) => change({ titlebarHueShift })}
          />
        }
      />
      <SettingRow
        stacked
        label="原创装饰"
        control={
          <div
            className="st-custom-decorations"
            role="radiogroup"
            aria-label="原创装饰样式"
          >
            {THEME_DECORATIONS.map((decoration) => (
              <label
                key={decoration}
                className="st-custom-decoration"
                data-active={custom.decoration === decoration}
              >
                <input
                  type="radio"
                  name="custom-theme-decoration"
                  value={decoration}
                  checked={custom.decoration === decoration}
                  onChange={() => change({ decoration })}
                />
                <span className="st-custom-decoration__art" aria-hidden="true">
                  <ThemeArtwork
                    accent="custom"
                    placement="titlebar"
                    decoration={decoration}
                  />
                  {decoration === "none" ? (
                    <span className="st-custom-decoration__empty" />
                  ) : null}
                </span>
                <span>{DECORATION_LABELS[decoration]}</span>
              </label>
            ))}
          </div>
        }
      />
      <SettingRow
        fill
        label="装饰强度"
        control={
          <Slider
            min={CUSTOM_THEME_BOUNDS.decorationOpacity[0] * 100}
            max={CUSTOM_THEME_BOUNDS.decorationOpacity[1] * 100}
            step={1}
            suffix="%"
            value={Math.round(custom.decorationOpacity * 100)}
            disabled={custom.decoration === "none"}
            aria-label="装饰强度"
            onChange={(value) => change({ decorationOpacity: value / 100 })}
          />
        }
      />
      <div className="st-custom-footer">
        <Button
          variant="secondary"
          size="sm"
          onClick={() => change({ ...DEFAULT_CUSTOM_THEME })}
        >
          重置自定义参数
        </Button>
      </div>
      <div className="st-preview-wrap">
        <div className="st-preview-wrap__hint nw-eyebrow">实时预览</div>
        <ThemePreview />
      </div>
    </SettingsGroup>
  );
}
