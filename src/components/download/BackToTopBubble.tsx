import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { IconArrowUp } from "./icons";
import { Tooltip } from "../ui";
import { useDownloadCenter, isDownloadBubbleVisible } from "./store";
import "./bubbles.css";
export interface BackToTopBubbleProps {
  target?: HTMLElement | null;
  threshold?: number;
  label?: string;
}
export function BackToTopBubble({ target, threshold = 420, label = "回到顶部" }: BackToTopBubbleProps) {
  const center = useDownloadCenter();
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!target) { setVisible(false); return; }
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setVisible(target.isConnected && target.scrollTop > threshold));
    };
    update();
    target.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    const observer = new ResizeObserver(update);
    observer.observe(target);
    for (const child of target.children) observer.observe(child);
    return () => {
      cancelAnimationFrame(frame); observer.disconnect();
      target.removeEventListener("scroll", update); window.removeEventListener("resize", update);
      // Cancel a smooth scroll before navigation restores another page's position.
      target.scrollTo({ top: target.scrollTop, behavior: "instant" });
    };
  }, [target, threshold]);
  if (!target) return null;
  return createPortal(<Tooltip label={label}>
    <button type="button" className="dl-top" data-visible={visible} data-download={isDownloadBubbleVisible(center)} aria-label={label} aria-hidden={!visible} tabIndex={visible ? 0 : -1} onClick={() => {
      target.dispatchEvent(new Event("resource-scroll-cancel"));
      target.scrollTo({ top: 0, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth" });
    }}>
      <span className="dl-top__glyph" aria-hidden="true"><IconArrowUp /></span>
    </button>
  </Tooltip>, document.body);
}
