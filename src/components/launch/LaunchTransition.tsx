import { useEffect, useLayoutEffect, useState, type ReactNode } from "react";
import { useVisualMotionPaused } from "../../lib/visualMotion";
import { Collapse } from "../ui/Collapse";

// Keep the last visible content intact while its container fades out.
export function RetainedLaunchContent({
  visible,
  children,
}: {
  visible: boolean;
  children: ReactNode;
}) {
  const [retained, setRetained] = useState<ReactNode>(null);
  const paused = useVisualMotionPaused();
  useLayoutEffect(() => {
    if (visible) setRetained(children);
  }, [visible, children]);
  useEffect(() => {
    if (visible) return;
    if (paused) {
      setRetained(null);
      return;
    }
    // Covers the panel's 260ms exit and the disclosure's 380ms collapse.
    const timer = window.setTimeout(() => setRetained(null), 400);
    return () => window.clearTimeout(timer);
  }, [visible, paused]);
  return visible ? children : retained;
}

export function LaunchReveal({
  visible,
  className,
  children,
}: {
  visible: boolean;
  className: string;
  children: ReactNode;
}) {
  return (
    <Collapse open={visible} className={`launch-reveal ${className}`}>
      <RetainedLaunchContent visible={visible}>
        {children}
      </RetainedLaunchContent>
    </Collapse>
  );
}
