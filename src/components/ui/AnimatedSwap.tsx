import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type Key,
  type ReactNode,
} from "react";

/** Fallback duration (ms) used when the CSS animation is skipped. */
const DEFAULT_DURATION = 200;
const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

function subscribeReducedMotion(onChange: () => void) {
  const media = window.matchMedia(REDUCED_MOTION_QUERY);
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}

function getReducedMotion() {
  return window.matchMedia(REDUCED_MOTION_QUERY).matches;
}

export interface AnimatedSwapProps {
  /** Identity of the current content. When it changes, a cross-fade plays. */
  swapKey: Key;
  children: ReactNode;
  /** How long the outgoing layer lingers before being removed (ms). */
  duration?: number;
  className?: string;
}

interface Outgoing {
  key: Key;
  node: ReactNode;
}

/**
 * Cross-fades swapped content: on a `swapKey` change the previous children are
 * briefly kept in an absolutely-positioned "out" layer (fading/rising out)
 * while the new children play their "in" animation. Pure CSS — no animation
 * dependency.
 *
 * Layout effects are used so the outgoing snapshot is committed *before* the
 * browser paints, which keeps the transition seamless.
 */
export function AnimatedSwap({
  swapKey,
  children,
  duration = DEFAULT_DURATION,
  className,
}: AnimatedSwapProps) {
  const reducedMotion = useSyncExternalStore(
    subscribeReducedMotion,
    getReducedMotion
  );
  const [outgoing, setOutgoing] = useState<Outgoing | null>(null);
  const prevKeyRef = useRef<Key>(swapKey);
  const prevNodeRef = useRef<ReactNode>(children);

  // 减弱动效时直接切页；切换系统偏好也立即丢弃正在退出的旧层。
  useLayoutEffect(() => {
    if (reducedMotion) setOutgoing(null);
    if (prevKeyRef.current === swapKey) return;
    if (!reducedMotion) {
      setOutgoing({ key: prevKeyRef.current, node: prevNodeRef.current });
    }
    prevKeyRef.current = swapKey;
  }, [swapKey, reducedMotion]);

  // Track the latest committed children for the *next* transition.
  useLayoutEffect(() => {
    prevNodeRef.current = children;
  });

  // Drop the outgoing layer once its exit animation has finished.
  useEffect(() => {
    if (!outgoing) return;
    const timer = window.setTimeout(() => setOutgoing(null), duration);
    return () => window.clearTimeout(timer);
  }, [outgoing, duration]);

  return (
    <div className={["ni-swap", className].filter(Boolean).join(" ")}>
      {outgoing && !reducedMotion ? (
        <div
          key={`out:${String(outgoing.key)}`}
          className="ni-swap__layer ni-swap__layer--out"
          aria-hidden="true"
        >
          {outgoing.node}
        </div>
      ) : null}
      <div
        key={`in:${String(swapKey)}`}
        className="ni-swap__layer ni-swap__layer--in"
      >
        {children}
      </div>
    </div>
  );
}

export default AnimatedSwap;
