import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react";

/** 滑动胶囊的几何信息（相对容器左上角）。 */
export interface SlidePill {
  /** 相对容器左边的位移（px）。 */
  left: number;
  /** 胶囊宽度（px）。 */
  width: number;
  /** 首次测量完成前为 false —— 用于先隐藏再淡入，避免从 (0,0) 处闪一下。 */
  ready: boolean;
}

export interface UseSlidePill<C extends HTMLElement, T extends HTMLElement> {
  /** 挂到容纳所有页签的容器（顶栏 nav / 首页分段容器）上。 */
  navRef: RefObject<C | null>;
  /** 为第 index 个页签生成 ref 回调：`ref={registerTab(i)}`。 */
  registerTab: (index: number) => (el: T | null) => void;
  /** 当前激活页签对应的胶囊几何信息。 */
  pill: SlidePill;
}

/**
 * 「滑动胶囊」动画机制：一个共享胶囊在激活页签之间滑动 + 伸缩，而不是让高亮块瞬移。
 * 从 TitleBar 抽出后，顶栏导航与首页的账号形态分段共用同一套测量逻辑，
 * 动效曲线 / 时长也因此完全一致（胶囊外观各自在 CSS 中定义）。
 *
 * 为什么用 ResizeObserver 而不是只监听 window.resize：页签宽度会随字体加载、
 * 文本变化与容器自身尺寸变化而改变，仅监听窗口不足以覆盖。
 *
 * 泛型 C / T 分别表示「容器元素」与「页签元素」：两者常常不同
 * （例如容器是 div、页签是 button），分开标注才能同时满足两侧 ref 的类型约束。
 */
export function useSlidePill<
  C extends HTMLElement = HTMLElement,
  T extends HTMLElement = HTMLElement,
>(activeIndex: number): UseSlidePill<C, T> {
  const navRef = useRef<C | null>(null);
  const tabRefs = useRef<Array<T | null>>([]);
  const tabCallbacks = useRef<Array<(el: T | null) => void>>([]);
  const [pill, setPill] = useState<SlidePill>({ left: 0, width: 0, ready: false });

  const measure = useCallback(() => {
    const nav = navRef.current;
    const tab = tabRefs.current[activeIndex];
    if (!nav || !tab) return;
    const navRect = nav.getBoundingClientRect();
    const tabRect = tab.getBoundingClientRect();
    setPill({
      left: tabRect.left - navRect.left,
      width: tabRect.width,
      ready: true,
    });
  }, [activeIndex]);

  // 绘制前先定位胶囊，避免出现「先落在第 0 项、再跳到目标项」的闪动。
  useLayoutEffect(() => {
    measure();
  }, [measure]);

  useEffect(() => {
    const nav = navRef.current;
    if (!nav) return;
    // 不支持 ResizeObserver 的内核退化为只监听窗口尺寸：胶囊仍可用，只是不跟随容器变化。
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", measure);
      return () => window.removeEventListener("resize", measure);
    }
    const observer = new ResizeObserver(() => measure());
    observer.observe(nav);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [measure]);

  // 每个下标复用同一个 ref 回调，避免每次渲染都生成新函数导致 ref 反复挂载 / 卸载。
  const registerTab = useCallback((index: number) => {
    const cached = tabCallbacks.current[index];
    if (cached) return cached;
    const callback = (el: T | null) => {
      tabRefs.current[index] = el;
    };
    tabCallbacks.current[index] = callback;
    return callback;
  }, []);

  return { navRef, registerTab, pill };
}
