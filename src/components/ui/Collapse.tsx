import { useLayoutEffect, useRef, useState, type HTMLAttributes } from "react";
import "./Collapse.css";

export interface CollapseProps extends HTMLAttributes<HTMLDivElement> {
  open: boolean;
  /**
   * 高度与内容错峰：展开时先撑高、内容稍后淡入；收起时内容先退场、高度稍后收拢。
   * 用于卡片内的条件区块，避免信息瞬间出现 / 消失造成卡片硬伸缩。
   */
  staggered?: boolean;
}

/** 测量实际内容高度参与布局，后续卡片随高度连续移动；收起即退出键盘序列。 */
export function Collapse({
  open,
  staggered = false,
  children,
  className,
  style,
  ...rest
}: CollapseProps) {
  const contentRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(0);
  useLayoutEffect(() => {
    const content = contentRef.current;
    if (!content) return;
    const measure = () => setHeight(content.getBoundingClientRect().height);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(content);
    return () => observer.disconnect();
  }, []);
  return (
    <div
      {...rest}
      className={[
        "nw-collapse",
        staggered && "nw-collapse--staggered",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      data-open={open}
      inert={!open}
      aria-hidden={!open}
      style={{ ...style, height: open ? height : 0 }}
    >
      <div ref={contentRef} className="nw-collapse__content">
        {children}
      </div>
    </div>
  );
}
