import type { CSSProperties } from "react";
import { useBackgroundSrc, useSettings } from "../../lib/settings";

/**
 * Global full-window background engine.
 *
 * Rendered once by the app shell; because it reads `settings` from context it
 * re-renders live whenever the settings page edits background/theme values —
 * that IS the preview mechanism, no separate preview component needed.
 * It is absolutely positioned inside the rounded shell so artwork is clipped
 * to the window radius (a `fixed` layer would show square corners on a
 * frameless transparent window).
 */
export function BackgroundLayer() {
  const { settings } = useSettings();
  const url = useBackgroundSrc(settings.backgroundImage);

  const style = {
    "--_bg-opacity": url ? String(settings.backgroundOpacity) : "0",
    "--_bg-blur": `${settings.backgroundBlur}px`,
    "--_bg-scrim": url ? "1" : "0",
  } as CSSProperties;

  return (
    <div className="nw-bg" aria-hidden="true" style={style}>
      <div className="nw-bg__mesh" />
      {url ? (
        <div
          className="nw-bg__image"
          style={{ backgroundImage: `url("${url}")` }}
        />
      ) : null}
      <div className="nw-bg__scrim" />
      <div className="nw-bg__grain" />
    </div>
  );
}
