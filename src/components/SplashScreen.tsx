import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "../lib/settings";

/** 仅兜底缺失的 transitionend；不是启动画面的最短展示时间。 */
const EXIT_FALLBACK_MS = 900;
/**
 * 等窗口真正可见的兜底时长。
 *
 * 窗口以 `visible: false` 创建（见 tauri.conf.json），页面加载完成后才显示。
 * 正常情况下 `visibilitychange` / `isVisible()` 会在显示时给出信号；这里只防
 * 「窗口一直不显示」导致外壳永远不交接。超过该时长仍不可见时只放行外壳，
 * 闪屏保持原样，等真实可见后再淡出。
 */
const REVEAL_FALLBACK_MS = 3000;
/** 窗口可见性轮询间隔：窗口显示后尽快交接，避免闪屏在隐藏状态下播完。 */
const VISIBILITY_POLL_MS = 140;
/** 3 秒兜底后放慢轮询，继续等待真实可见；不能停止，否则可能永远不淡出。 */
const SLOW_VISIBILITY_POLL_MS = 1000;
/** 单次 `isVisible()` 查询的超时：超时按本次失败处理并继续轮询，避免 Promise 悬挂导致轮询链断掉。 */
const VISIBILITY_QUERY_TIMEOUT_MS = 1500;
/**
 * 绝对硬截止：无论是否等到真实的「可见」信号，都必须交接外壳并淡出启动画面。
 *
 * 正常路径仍等真实可见后再淡出；硬截止只保证应用最终可用，避免窗口 API
 * 永久无响应等启动期死锁。3 秒兜底仍只解锁外壳并保持遮挡，不代替本截止。
 */
const HARD_REVEAL_DEADLINE_MS = 6000;
const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

/**
 * 当前窗口句柄的惰性单例。
 *
 * `@tauri-apps/api` 的 getCurrentWindow() 会同步读取注入的
 * `__TAURI_INTERNALS__.metadata.currentWindow`；字段缺失（测试替身、旧内核）时
 * 会直接抛 TypeError，Promise 的 .catch 接不住。因此解析动作必须包在同步
 * try/catch 里，成功结果只取一次并缓存，避免每轮轮询重复解析。
 */
let currentWindow: ReturnType<typeof getCurrentWindow> | null = null;
function resolveCurrentWindow(): ReturnType<typeof getCurrentWindow> | null {
  if (currentWindow) return currentWindow;
  try {
    currentWindow = getCurrentWindow();
  } catch {
    // 解析失败不缓存：下一轮轮询仍会重试，期间绝不据此淡出。
    currentWindow = null;
  }
  return currentWindow;
}

/**
 * 查询当前窗口是否可见，并给 Promise 加超时竞速。
 *
 * - `true` / `false`：窗口 API 给出的可信结果；
 * - `null`：本次查询失败（同步抛错、reject、非布尔返回值或 1500ms 内无响应）。
 *
 * 无论超时还是先有结果都会清理计时器，避免悬挂查询留下多余定时器。
 * 超时只是「这次没问出来」，调用方必须继续轮询，不能据此淡出。
 */
function probeWindowVisibility(
  win: ReturnType<typeof getCurrentWindow>,
  timers: Set<number>
): Promise<boolean | null> {
  return new Promise((resolve) => {
    let settled = false;
    let timer: number | undefined;

    const settle = (value: boolean | null) => {
      if (settled) return;
      settled = true;
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timers.delete(timer);
      }
      resolve(value);
    };

    timer = window.setTimeout(() => settle(null), VISIBILITY_QUERY_TIMEOUT_MS);
    // 登记在途查询的超时计时器：卸载 / 交接时统一清理，StrictMode 重入不残留。
    timers.add(timer);

    let pending: Promise<unknown>;
    try {
      pending = win.isVisible();
    } catch {
      // 句柄失效时也可能同步抛错：同样视为本次失败。
      settle(null);
      return;
    }

    pending.then(
      (value) => settle(typeof value === "boolean" ? value : null),
      () => settle(null)
    );
  });
}

/**
 * index.html 在脚本载入前绘制启动画面；React 提交外壳后即已具备可交互首屏。
 * 后续设置、账号与版本请求由各页面反馈，不阻塞整窗，也不等待装饰动画播完。
 */
export function SplashScreen() {
  useEffect(() => {
    const root = document.documentElement;
    const el = document.getElementById("nw-splash");
    let released = false;
    let disposed = false;
    let revealTimer: number | undefined;
    let pollTimer: number | undefined;
    let hardRevealTimer: number | undefined;
    let disposeFade: (() => void) | undefined;
    let pollSequence = 0;
    const startedAt = Date.now();
    /** 在途 isVisible 查询的超时计时器：卸载 / 交接时一并清理。 */
    const probeTimers = new Set<number>();

    /** 清理启动交接的全部观察器与在途查询计时器；重复调用安全。 */
    const stopWatching = () => {
      window.clearTimeout(revealTimer);
      window.clearTimeout(pollTimer);
      window.clearTimeout(hardRevealTimer);
      for (const timer of probeTimers) window.clearTimeout(timer);
      probeTimers.clear();
      document.removeEventListener("visibilitychange", onVisible);
    };

    /**
     * 启动遮挡期间冻结整个应用。
     *
     * 仅靠 `pointer-events: none` 只能挡住鼠标 / 触摸，键盘 Tab 与辅助技术仍可
     * 进入被闪屏覆盖的控件；`inert` 同时阻止焦点与交互。
     *
     * 目标必须是 `document.body`：Modal / Tooltip / Toast 等 portal 直接挂到
     * body，不在 `.nw-shell` 内，只惰性化外壳会漏掉这些弹窗。`#nw-splash` 也是
     * body 子节点，但它只是纯视觉层（`pointer-events: none`、无交互），inert
     * 不影响其淡出过渡。使用 setAttribute / removeAttribute 而非 `element.inert`，
     * 避免不同 TS DOM 版本的类型差异。
     */
    const setAppInert = (inert: boolean) => {
      if (inert) document.body.setAttribute("inert", "");
      else document.body.removeAttribute("inert");
    };

    /**
     * 交接：放行外壳并淡出启动画面。
     *
     * 只有确认窗口真实可见后才调用；此时才开始淡出，避免闪屏在用户看不到的
     * 时候播完并被移除，启动时只剩「应用本体突然出现」。
     */
    const startFade = () => {
      if (released) return;
      released = true;
      stopWatching();
      // 交接后解除启动遮挡：整个应用（含 body 级 portal）恢复指针与键盘交互。
      setAppInert(false);
      root.classList.add("nw-app-ready");
      // 兜底可能已给外壳加过 hold；淡出开始后解除，让外壳恢复交互。
      root.classList.remove("nw-splash-hold");
      if (!el) return;

      const reducedMotion = window.matchMedia(REDUCED_MOTION_QUERY);
      let fallbackTimer: number | undefined;
      const cleanup = () => {
        window.clearTimeout(fallbackTimer);
        el.removeEventListener("transitionend", onTransitionEnd);
        el.removeEventListener("transitioncancel", onTransitionEnd);
        reducedMotion.removeEventListener("change", onMotionChange);
      };
      const finish = () => {
        cleanup();
        el.remove();
      };
      const onTransitionEnd = (event: TransitionEvent) => {
        if (event.target === el && event.propertyName === "opacity") finish();
      };
      const onMotionChange = () => {
        if (reducedMotion.matches) finish();
      };

      disposeFade = cleanup;
      el.addEventListener("transitionend", onTransitionEnd);
      el.addEventListener("transitioncancel", onTransitionEnd);
      reducedMotion.addEventListener("change", onMotionChange);
      el.classList.add("nw-splash--leaving");

      const style = getComputedStyle(el);
      const durations = style.transitionDuration.split(",").map(parseFloat);
      const hasFade = style.transitionProperty
        .split(",")
        .some(
          (property, index) =>
            ["opacity", "all"].includes(property.trim()) &&
            durations[index % durations.length] > 0
        );
      if (reducedMotion.matches || !hasFade) {
        finish();
      } else {
        fallbackTimer = window.setTimeout(finish, EXIT_FALLBACK_MS);
      }
    };

    /**
     * visibilitychange 快速路径：只把「变为可见」当作触发信号。
     * Tauri 隐藏窗口下 document.hidden 并不可靠，不能据此交接；
     * Tauri 路径改为立即用窗口 API 复核，只有普通浏览器才读 document.hidden。
     */
    const onVisible = () => {
      if (disposed || released) return;
      if (isTauri()) {
        window.clearTimeout(pollTimer);
        pollWindowVisibility();
        return;
      }
      if (!document.hidden) startFade();
    };

    /**
     * 轮询窗口真实可见性：Tauri 以 `visible: false` 创建窗口时，
     * document.visibilityState 可能仍是 visible，只有窗口 API 能给出真实状态。
     * 3 秒内用较短间隔尽快交接；之后放慢到 1 秒继续等待，不能停止轮询 ——
     * WebView2 不一定补发 visibilitychange，停止后闪屏可能永远不淡出。
     *
     * 每次查询都有 1500ms 超时兜底：isVisible() 悬挂时本轮按失败处理并继续
     * 轮询，轮询链不会断；真正的最终保障由 HARD_REVEAL_DEADLINE_MS 提供。
     */
    const pollWindowVisibility = () => {
      if (disposed || released) return;
      // 每次轮询生成新序号：visibilitychange 触发的即时复核会让在途结果失效，
      // 避免多条定时器链并行轮询。
      const pollId = ++pollSequence;
      const nextDelay = () =>
        Date.now() - startedAt < REVEAL_FALLBACK_MS
          ? VISIBILITY_POLL_MS
          : SLOW_VISIBILITY_POLL_MS;

      const win = resolveCurrentWindow();
      if (!win) {
        // 取不到窗口句柄（metadata 缺失等）：Tauri 下无法确认可见性，
        // 只安排慢速重试，绝不据此淡出，避免窗口仍隐藏时闪屏提前播完。
        pollTimer = window.setTimeout(
          pollWindowVisibility,
          SLOW_VISIBILITY_POLL_MS
        );
        return;
      }

      void probeWindowVisibility(win, probeTimers).then((visible) => {
        // 在途结果可能已被新的 visibilitychange 复核、卸载或交接淘汰：
        // 此时绝不能再安排定时器，否则会留下多余的轮询链。
        if (disposed || released || pollId !== pollSequence) return;
        if (visible === true) {
          startFade();
          return;
        }
        // false 是可信的不可见结果，按当前阶段节奏继续轮询；
        // null 表示本次查询失败 / 超时，放慢重试直到拿到明确结果。
        pollTimer = window.setTimeout(
          pollWindowVisibility,
          visible === false ? nextDelay() : SLOW_VISIBILITY_POLL_MS
        );
      });
    };

    // 启动遮挡期间冻结整个应用（含 body 级 portal 弹窗）：闪屏覆盖在应用之上时，
    // 应用既不应响应指针，也不应被键盘 / 辅助技术聚焦。
    setAppInert(true);

    document.addEventListener("visibilitychange", onVisible);

    if (isTauri()) {
      pollWindowVisibility();
    } else if (!document.hidden) {
      // 普通浏览器没有窗口 API：document.hidden 是唯一可用的可见性信号。
      startFade();
    }

    revealTimer = window.setTimeout(() => {
      if (disposed || released) return;
      // 3 秒仍不可见：只解锁外壳透明度，不添加 nw-splash--leaving、不删除闪屏。
      // 这样窗口真正显示时仍能看到完整的启动画面，再由可见信号触发淡出。
      // nw-splash-hold 让「已解锁但仍被闪屏覆盖」的外壳不抢点击；整个应用的
      // inert 保持到 startFade / 硬截止再解除。
      root.classList.add("nw-app-ready", "nw-splash-hold");
    }, REVEAL_FALLBACK_MS);

    hardRevealTimer = window.setTimeout(() => {
      if (disposed || released) return;
      // 正常路径仍等真实可见；硬截止只保证应用最终可用，避免启动期死锁。
      // 即使 isVisible() 永久悬挂或窗口从不给出可见信号，也在这里解除
      // nw-splash-hold、移除整个应用的 inert 并淡出闪屏。
      startFade();
    }, HARD_REVEAL_DEADLINE_MS);

    return () => {
      disposed = true;
      stopWatching();
      disposeFade?.();
      // StrictMode 会清理后立即重跑 effect：这里必须解冻整个应用，否则重跑前
      // 的空档期应用保持 inert；重跑时若仍在遮挡会再次添加。
      setAppInert(false);
    };
  }, []);

  return null;
}

export default SplashScreen;
