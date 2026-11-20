import { useCallback, useEffect, useRef, useState } from "react";
import { IconArrowUp } from "./icons";
import "./bubbles.css";

/** 默认出现阈值（滚动像素）。 */
const DEFAULT_THRESHOLD = 420;

function isScrollable(node: HTMLElement): boolean {
  const style = window.getComputedStyle(node);
  const scrollableY = /(auto|scroll|overlay)/.test(style.overflowY);
  return scrollableY && node.scrollHeight > node.clientHeight + 1;
}

/**
 * 找到最近的「可纵向滚动祖先」。
 *
 * 为什么向上找而不是写死 `.ni-content`：本项目所有页面都渲染在滚动容器内，
 * 但容器类名属于外壳模块；按祖先链探测可以让本组件在任何容器里都直接可用，
 * 找不到时回退到窗口滚动。
 */
function findScroller(start: HTMLElement | null): HTMLElement | null {
  let node = start?.parentElement ?? null;
  while (node) {
    if (isScrollable(node)) return node;
    node = node.parentElement;
  }
  return null;
}

function scrollerTop(scroller: HTMLElement | null): number {
  if (scroller) return scroller.scrollTop;
  return window.scrollY || document.documentElement.scrollTop || 0;
}

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

export interface BackToTopBubbleProps {
  /** 页面向下滚动超过该像素数后出现。 */
  threshold?: number;
  /** 无障碍名称（气泡内不显示文字）。 */
  label?: string;
}

/**
 * 「回到顶部」气泡浮窗（可复用）。
 *
 * 图标型按钮不占正文空间；页面滚动超过阈值后出现在右下角，
 * 点击平滑滚回顶部（`prefers-reduced-motion` 下改为瞬时跳转）。
 */
export function BackToTopBubble({
  threshold = DEFAULT_THRESHOLD,
  label = "回到顶部",
}: BackToTopBubbleProps) {
  const ref = useRef<HTMLButtonElement | null>(null);
  const scrollerRef = useRef<HTMLElement | null>(null);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    const update = () => {
      if (!scrollerRef.current && ref.current) {
        scrollerRef.current = findScroller(ref.current);
      }
      setVisible(scrollerTop(scrollerRef.current) > threshold);
    };

    update();
    // scroll 事件不冒泡，但捕获阶段能收到任意内部容器的滚动。
    document.addEventListener("scroll", update, { capture: true, passive: true });
    window.addEventListener("resize", update);
    return () => {
      document.removeEventListener("scroll", update, { capture: true });
      window.removeEventListener("resize", update);
    };
  }, [threshold]);

  const handleClick = useCallback(() => {
    const behavior: ScrollBehavior = prefersReducedMotion() ? "auto" : "smooth";
    const scroller = scrollerRef.current ?? findScroller(ref.current);
    if (scroller) {
      scroller.scrollTo({ top: 0, behavior });
      return;
    }
    window.scrollTo({ top: 0, behavior });
  }, []);

  if (!visible) return null;

  return (
    <button
      type="button"
      ref={ref}
      className="dl-top"
      aria-label={label}
      onClick={handleClick}
    >
      <span className="dl-top__glyph" aria-hidden="true">
        <IconArrowUp />
      </span>
    </button>
  );
}
