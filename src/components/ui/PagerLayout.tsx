import { useLayoutEffect, useRef, type ReactNode } from "react";
import { AnimatedSwap } from "./AnimatedSwap";
import "./pager.css";

/**
 * PagerLayout —— 「侧边栏选项 ↔ 右侧独立页」的通用页面骨架。
 *
 * 设计约定（R2 冻结）：
 *   - 侧边栏是**页面切换器**，不是滚动锚点导航：点击某个选项 = 右侧整页替换。
 *   - 右侧内容区自行滚动（内容可能很长），侧边栏保持固定，不随内容滚动。
 *   - 视觉语言与设置页既有的 .st-nav 一致：填充式圆角、激活项用颜色主题色（accent）柔光底，
 *     不使用 1px 描边作为主表现。
 *
 * 该组件由 Lead 作为「共享基础件」提供，任何页面都可以复用（设置 / 资源 / 版本设置 / 更多…）。
 */
export interface PagerItem {
  /** 唯一 id，用于 `active` 比较。 */
  id: string;
  /** 选项文案（中文）。 */
  label: string;
  /** 可选图标（建议 16–18px 线性图标）。 */
  icon?: ReactNode;
  /** 可选分组标题：与前一项分组不同时，在该项上方渲染一行分组标题。 */
  group?: string;
}

export interface PagerLayoutProps {
  items: PagerItem[];
  active: string;
  onSelect: (id: string) => void;
  /** 侧边栏的无障碍标签，例如「设置分区」。 */
  navLabel: string;
  /** 侧边栏顶部的分组标题（可选）。 */
  navTitle?: ReactNode;
  /** 右侧独立页内容。 */
  children: ReactNode;
  /** 额外 class（可选）。 */
  className?: string;
}

export function PagerLayout({
  items,
  active,
  onSelect,
  navLabel,
  navTitle,
  children,
  className,
}: PagerLayoutProps) {
  const pageRef = useRef<HTMLElement>(null);

  // 分区是整页替换而不是锚点跳转：新页必须先回到顶部，再参与 keyed 过渡。
  useLayoutEffect(() => {
    const page = pageRef.current;
    if (page) page.scrollTop = 0;
  }, [active]);

  return (
    <div className={className ? `ni-pager ${className}` : "ni-pager"}>
      <nav className="ni-pager__nav" aria-label={navLabel}>
        {navTitle !== undefined ? (
          <div className="ni-pager__title ni-eyebrow">{navTitle}</div>
        ) : null}
        <ul className="ni-pager__list">
          {items.map((item, index) => {
            const isActive = item.id === active;
            const group = item.group;
            const prevGroup = index > 0 ? items[index - 1].group : undefined;
            const showGroup = group !== undefined && group !== prevGroup;
            return (
              <li key={item.id}>
                {showGroup ? (
                  <div className="ni-pager__group">{group}</div>
                ) : null}
                <button
                  type="button"
                  className="ni-pager__item"
                  data-active={isActive}
                  aria-current={isActive ? "true" : undefined}
                  onClick={() => {
                    if (!isActive) onSelect(item.id);
                  }}
                >
                  {item.icon !== undefined ? (
                    <span className="ni-pager__icon" aria-hidden="true">
                      {item.icon}
                    </span>
                  ) : null}
                  <span className="ni-pager__label">{item.label}</span>
                </button>
              </li>
            );
          })}
        </ul>
      </nav>
      <section ref={pageRef} className="ni-pager__page" aria-live="polite">
        <AnimatedSwap swapKey={active} className="ni-pager__swap">
          {children}
        </AnimatedSwap>
      </section>
    </div>
  );
}

export default PagerLayout;
