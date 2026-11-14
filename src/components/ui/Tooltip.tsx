import {
  cloneElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent as ReactFocusEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
  type Ref,
} from "react";
import { createPortal } from "react-dom";

/* ============================================================================
   自绘悬浮提示 · Tooltip
   原生 title 的替代品：可动画、可键盘触达、可被主题化。
   设计语言：填充式气泡 + 柔和分层阴影（不做 1px 描边）；颜色 / 圆角 / 时长
   全部取自 design tokens（样式见 src/styles/ui.css）。

   为什么用 cloneElement 而不是包一层 div：Tooltip 会被挂到按钮、色板、
   图标按钮等既有元素上，额外插入包裹节点会改变 flex/grid 布局（尤其是
   100% 宽度的控件），所以这里把事件与 aria 直接克隆到触发元素本身。
   定位（R9）：指针悬浮时气泡出现在鼠标右下角并跟随指针（rAF 节流）；
   键盘 focus-visible 无指针坐标，回退到触发元素附近的 top/bottom 定位。
   ========================================================================== */

export type TooltipSide = "top" | "bottom";

/** 内部定位形态：键盘回退沿用 top/bottom；指针跟随统一为 pointer（翻转轴用 data-flip 表达）。 */
type TooltipPlacement = TooltipSide | "pointer";

/**
 * 触发元素需要能接收的 props。
 * 用「方法签名」而非函数属性来声明，以获得参数双变（bivariance）：
 * 这样宿主元素（button / div / span / a）与 forwardRef 组件都能直接传入。
 */
type TooltipChildProps = {
  ref?: Ref<HTMLElement>;
  title?: string;
  "aria-describedby"?: string;
  onPointerEnter?(event: ReactPointerEvent<HTMLElement>): void;
  onPointerMove?(event: ReactPointerEvent<HTMLElement>): void;
  onPointerLeave?(event: ReactPointerEvent<HTMLElement>): void;
  onFocus?(event: ReactFocusEvent<HTMLElement>): void;
  onBlur?(event: ReactFocusEvent<HTMLElement>): void;
  onKeyDown?(event: ReactKeyboardEvent<HTMLElement>): void;
};

export interface TooltipProps {
  /** 提示文案（中文 UI 文案）。 */
  label: string;
  /** 触发元素，必须是单个元素（类名 / 事件会被克隆到它身上）。 */
  children: ReactElement<TooltipChildProps>;
  /** 键盘回退的首选方位；该方向空间不足时自动翻转到另一侧。指针跟随不受此影响。 */
  side?: TooltipSide;
  /** 指针悬浮多久后出现（毫秒）；键盘 focus-visible 立即出现。 */
  delay?: number;
}

/** 与 CSS 里 .nw-tooltip 的 --tip-gap / --tip-edge 对应，读不到时用同值兜底。 */
const FALLBACK_GAP = 8;
const FALLBACK_EDGE = 8;
/** 默认延迟：轻扫不打扰，停留片刻才提示。 */
const DEFAULT_DELAY = 350;

function cssPx(el: HTMLElement, name: string, fallback: number): number {
  const raw = window.getComputedStyle(el).getPropertyValue(name).trim();
  const value = Number.parseFloat(raw);
  return Number.isFinite(value) ? value : fallback;
}

function assignRef<T>(ref: Ref<T> | undefined, value: T | null): void {
  if (typeof ref === "function") {
    ref(value);
    return;
  }
  if (ref) {
    // RefObject.current 在不同 React 版本里可能声明为只读，这里统一按可变引用写入。
    (ref as unknown as { current: T | null }).current = value;
  }
}

/** 指针坐标（视口坐标，对应 position:fixed 的气泡）。 */
type Point = { x: number; y: number };

export function Tooltip({
  label,
  children,
  side = "top",
  delay = DEFAULT_DELAY,
}: TooltipProps) {
  const tipId = useId();
  const triggerRef = useRef<HTMLElement | null>(null);
  const tipRef = useRef<HTMLDivElement | null>(null);
  const timerRef = useRef<number | null>(null);
  const rafRef = useRef<number | null>(null);
  // 最新指针坐标：pointerenter 立即记录，pointermove 经 rAF 节流提交。
  const pointerRef = useRef<Point | null>(null);
  const pendingRef = useRef<Point | null>(null);
  // open / 定位模式的 ref 镜像：供 rAF 回调读取最新值，避免闭包过期。
  const openRef = useRef(false);
  const modeRef = useRef<"pointer" | "focus">("focus");
  const [open, setOpen] = useState(false);
  const [placement, setPlacement] = useState<TooltipPlacement>(side);
  const [flip, setFlip] = useState("");
  const [coords, setCoords] = useState<{ top: number; left: number } | null>(
    null
  );

  openRef.current = open;

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const cancelRaf = useCallback(() => {
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    pendingRef.current = null;
  }, []);

  const hide = useCallback(() => {
    clearTimer();
    cancelRaf();
    setOpen(false);
  }, [clearTimer, cancelRaf]);

  /** 指针跟随：气泡默认在指针右下，越界时按轴翻转到左上，再钳制进视口。 */
  const positionAtPointer = useCallback(() => {
    const tip = tipRef.current;
    const pointer = pointerRef.current;
    if (!tip || !pointer) return;
    // 入场动画会缩放气泡；使用未变换的布局尺寸，避免右侧 / 底部越出视口。
    const width = tip.offsetWidth;
    const height = tip.offsetHeight;
    const gap = cssPx(tip, "--tip-gap", FALLBACK_GAP);
    const edge = cssPx(tip, "--tip-edge", FALLBACK_EDGE);
    const maxLeft = Math.max(edge, window.innerWidth - width - edge);
    const maxTop = Math.max(edge, window.innerHeight - height - edge);
    let left = pointer.x + gap;
    let top = pointer.y + gap;
    let flipX = false;
    let flipY = false;
    if (left > maxLeft) {
      left = pointer.x - gap - width;
      flipX = true;
    }
    if (top > maxTop) {
      top = pointer.y - gap - height;
      flipY = true;
    }
    const nextFlip = [flipX ? "x" : "", flipY ? "y" : ""]
      .filter(Boolean)
      .join(" ");
    const nextLeft = Math.min(Math.max(left, edge), maxLeft);
    const nextTop = Math.min(Math.max(top, edge), maxTop);
    setPlacement("pointer");
    setFlip(nextFlip);
    // 坐标未变时复用旧对象：指针停顿时不做无谓重渲染。
    setCoords((prev) =>
      prev && prev.left === nextLeft && prev.top === nextTop
        ? prev
        : { left: nextLeft, top: nextTop }
    );
  }, []);

  /** 键盘回退：无指针坐标，沿用触发元素附近的 top/bottom 定位（无障碍硬约束）。 */
  const positionAtTrigger = useCallback(() => {
    const trigger = triggerRef.current;
    const tip = tipRef.current;
    if (!trigger || !tip) return;
    const triggerRect = trigger.getBoundingClientRect();
    const tipRect = { width: tip.offsetWidth, height: tip.offsetHeight };
    const gap = cssPx(tip, "--tip-gap", FALLBACK_GAP);
    const edge = cssPx(tip, "--tip-edge", FALLBACK_EDGE);
    const viewW = window.innerWidth;
    const viewH = window.innerHeight;

    let next: TooltipSide = side;
    if (side === "top" && triggerRect.top - tipRect.height - gap < edge) {
      next = "bottom";
    } else if (
      side === "bottom" &&
      triggerRect.bottom + tipRect.height + gap > viewH - edge
    ) {
      next = "top";
    }

    const preferredTop =
      next === "top"
        ? triggerRect.top - tipRect.height - gap
        : triggerRect.bottom + gap;
    const centered =
      triggerRect.left + triggerRect.width / 2 - tipRect.width / 2;
    const maxLeft = Math.max(edge, viewW - tipRect.width - edge);
    const maxTop = Math.max(edge, viewH - tipRect.height - edge);

    setPlacement(next);
    setFlip("");
    setCoords({
      top: Math.min(Math.max(preferredTop, edge), maxTop),
      left: Math.min(Math.max(centered, edge), maxLeft),
    });
  }, [side]);

  const show = useCallback(
    (immediate: boolean, mode: "pointer" | "focus") => {
      clearTimer();
      modeRef.current = mode;
      const reveal = () => {
        timerRef.current = null;
        // 已打开时（如指针悬浮中又键盘聚焦）直接按新模式重新定位。
        if (openRef.current) {
          if (mode === "pointer") positionAtPointer();
          else positionAtTrigger();
        }
        setOpen(true);
      };
      if (immediate) {
        reveal();
        return;
      }
      timerRef.current = window.setTimeout(reveal, delay);
    },
    [clearTimer, delay, positionAtPointer, positionAtTrigger]
  );

  // 关闭时丢弃旧坐标，避免下次出现时先闪在上一处位置。
  useEffect(() => {
    if (!open) setCoords(null);
  }, [open]);

  // 打开后测量：指针模式取最新指针坐标（延迟期内可能已移动），键盘模式回退触发元素。
  useLayoutEffect(() => {
    if (!open) return;
    if (modeRef.current === "pointer" && pointerRef.current) {
      positionAtPointer();
    } else {
      positionAtTrigger();
    }
  }, [open, label, side, positionAtPointer, positionAtTrigger]);

  // 打开期间：滚动 / 尺寸变化 / Escape 一律收起（提示不该停留在旧位置）。
  useEffect(() => {
    if (!open) return;
    const handleScroll = () => hide();
    const handleResize = () => hide();
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    window.addEventListener("scroll", handleScroll, true);
    window.addEventListener("resize", handleResize);
    document.addEventListener("keydown", handleKey);
    return () => {
      window.removeEventListener("scroll", handleScroll, true);
      window.removeEventListener("resize", handleResize);
      document.removeEventListener("keydown", handleKey);
    };
  }, [open, hide]);

  // 卸载时清掉尚未触发的定时器与 rAF。
  useEffect(
    () => () => {
      clearTimer();
      cancelRaf();
    },
    [clearTimer, cancelRaf]
  );

  const childProps = children.props;

  /** rAF 节流提交指针坐标：打开期间同步更新位置，未打开时只记录最新坐标。 */
  const commitPointer = useCallback(() => {
    rafRef.current = null;
    const pending = pendingRef.current;
    pendingRef.current = null;
    if (!pending) return;
    pointerRef.current = pending;
    if (openRef.current && modeRef.current === "pointer") {
      positionAtPointer();
    }
  }, [positionAtPointer]);

  const handlePointerEnter = (event: ReactPointerEvent<HTMLElement>) => {
    childProps.onPointerEnter?.(event);
    pointerRef.current = { x: event.clientX, y: event.clientY };
    show(false, "pointer");
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLElement>) => {
    childProps.onPointerMove?.(event);
    pendingRef.current = { x: event.clientX, y: event.clientY };
    if (rafRef.current === null) {
      rafRef.current = window.requestAnimationFrame(commitPointer);
    }
  };

  const handlePointerLeave = (event: ReactPointerEvent<HTMLElement>) => {
    childProps.onPointerLeave?.(event);
    hide();
  };

  const handleFocus = (event: ReactFocusEvent<HTMLElement>) => {
    childProps.onFocus?.(event);
    // 只有键盘 focus-visible 才弹提示：鼠标点按带来的聚焦不打扰用户。
    let focusVisible = true;
    try {
      focusVisible =
        event.target instanceof Element &&
        event.target.matches(":focus-visible");
    } catch {
      // 极老内核不支持该选择器时，退化为「始终提示」（仍是可访问的）。
      focusVisible = true;
    }
    if (focusVisible) show(true, "focus");
  };

  const handleBlur = (event: ReactFocusEvent<HTMLElement>) => {
    childProps.onBlur?.(event);
    hide();
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    childProps.onKeyDown?.(event);
    if (event.key === "Escape") hide();
  };

  // 仅在显示时挂 aria-describedby：id 稳定，但不在 DOM 里的说明不应被引用。
  const existingDescribedBy = childProps["aria-describedby"];
  const describedBy = open
    ? [existingDescribedBy, tipId].filter(Boolean).join(" ")
    : existingDescribedBy;

  const trigger = cloneElement(children, {
    ref: (node: HTMLElement | null) => {
      triggerRef.current = node;
      assignRef(children.props.ref, node);
    },
    title: undefined,
    "aria-describedby": describedBy,
    onPointerEnter: handlePointerEnter,
    onPointerMove: handlePointerMove,
    onPointerLeave: handlePointerLeave,
    onFocus: handleFocus,
    onBlur: handleBlur,
    onKeyDown: handleKeyDown,
  });

  return (
    <>
      {trigger}
      {open
        ? createPortal(
            <div
              ref={tipRef}
              id={tipId}
              role="tooltip"
              data-side={placement}
              data-flip={flip}
              className="nw-tooltip"
              style={{
                top: coords ? `${coords.top}px` : "0px",
                left: coords ? `${coords.left}px` : "0px",
                // 首帧尚未测量：先隐藏，避免在 (0,0) 处闪一下。
                visibility: coords ? "visible" : "hidden",
              }}
            >
              {label}
            </div>,
            document.body
          )
        : null}
    </>
  );
}
