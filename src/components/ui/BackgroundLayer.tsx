import type { CSSProperties } from "react";
import { useBackgroundSrc, useSettings } from "../../lib/settings";
import { ThemeArtwork } from "./ThemeArtwork";

/** A user image replaces theme artwork; all layers stay inside the clipped shell. */
export function BackgroundLayer() {
  const { settings } = useSettings();
  const url = useBackgroundSrc(settings.backgroundImage);
  const hasUserBackground = Boolean(settings.backgroundImage?.trim());
  const style = {
    "--_bg-opacity": url ? String(settings.backgroundOpacity) : "0",
    "--_bg-blur": `${settings.backgroundBlur}px`,
    "--_bg-scrim": url ? "1" : "0",
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
      {url ? (
        <div
          className="ni-bg__image"
          style={{ backgroundImage: `url(${JSON.stringify(url)})` }}
        />
      ) : null}
      <div className="ni-bg__scrim" />
      <div className="ni-bg__grain" />
    </div>
  );
}
