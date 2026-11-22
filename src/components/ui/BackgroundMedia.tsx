import { useEffect, useRef, type CSSProperties } from "react";
import { backgroundKind, useBackgroundSrc } from "../../lib/settings";
import { useVisualMotionPaused } from "../../lib/visualMotion";

type Props = {
  path: string;
  className?: string;
  style?: CSSProperties;
  animate?: boolean;
  onLoad?: () => void;
  onError?: () => void;
};

/** One renderer keeps the shell, live preview and picker consistent. */
export function BackgroundMedia({
  path,
  className,
  style,
  animate = true,
  onLoad,
  onError,
}: Props) {
  const { src, recover } = useBackgroundSrc(path);
  const videoRef = useRef<HTMLVideoElement>(null);
  const paused = useVisualMotionPaused() || !animate;
  const kind = backgroundKind(path);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (paused) video.pause();
    else void video.play().catch(() => undefined);
    return () => video.pause();
  }, [src, paused]);

  if (!src) return null;
  const common = { className, style, "aria-hidden": true as const };
  const showFirstFrame = () => {
    if (paused && videoRef.current) videoRef.current.pause();
    onLoad?.();
  };
  // Report an image error only once the one-time data URL fallback is exhausted.
  const failImage = () => {
    if (!recover(onError)) onError?.();
  };
  return kind === "video" ? (
    <video
      {...common}
      ref={videoRef}
      src={src}
      autoPlay={!paused}
      muted
      loop
      playsInline
      preload="auto"
      disablePictureInPicture
      tabIndex={-1}
      onLoadedData={showFirstFrame}
      onError={onError}
    />
  ) : (
    <img {...common} src={src} alt="" decoding="async" onLoad={onLoad} onError={failImage} />
  );
}
