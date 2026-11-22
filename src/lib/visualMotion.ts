import { useSyncExternalStore } from "react";

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

function subscribe(onChange: () => void): () => void {
  const media = window.matchMedia(REDUCED_MOTION);
  media.addEventListener("change", onChange);
  document.addEventListener("visibilitychange", onChange);
  return () => {
    media.removeEventListener("change", onChange);
    document.removeEventListener("visibilitychange", onChange);
  };
}

function snapshot(): boolean {
  return document.hidden || window.matchMedia(REDUCED_MOTION).matches;
}

/** Decorative motion never runs in a hidden window or against system preferences. */
export function useVisualMotionPaused(): boolean {
  return useSyncExternalStore(subscribe, snapshot, () => true);
}
