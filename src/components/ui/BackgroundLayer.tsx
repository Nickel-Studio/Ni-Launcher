import type { CSSProperties } from "react";
import { useSettings } from "../../lib/settings";
import { BackgroundMedia } from "./BackgroundMedia";
import { ThemeArtwork } from "./ThemeArtwork";

/** User media replaces theme backgrounds, but never the separate titlebar motif. */
export function BackgroundLayer() {
  const { settings } = useSettings();
  const path = settings.backgroundImage?.trim() || null;
  const hasUserBackground = Boolean(path);
  const style = {
    "--_bg-opacity": path ? String(settings.backgroundOpacity) : "0",
    "--_bg-blur": `${settings.backgroundBlur}px`,
    "--_bg-scrim": path ? "1" : "0",
  } as CSSProperties;

  return (
    <div
      className="ni-bg"
      aria-hidden="true"
      style={style}
      data-user-background={hasUserBackground}
    >
      <div className="ni-bg__mesh" />
      {!hasUserBackground ? (
        <ThemeArtwork
          accent={settings.accent}
          decoration={settings.customTheme.decoration}
        />
      ) : null}
      {path ? <BackgroundMedia key={path} path={path} className="ni-bg__image" /> : null}
      <div className="ni-bg__scrim" />
      <div className="ni-bg__grain" />
    </div>
  );
}
