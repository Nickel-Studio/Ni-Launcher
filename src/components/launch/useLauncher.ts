import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri, type SourcePolicy } from "../../lib/settings";

/** 后端 `launcher-event` 的原始 payload（字段做宽松兜底）。 */
type RawPayload = Record<string, unknown>;

function eventNumber(payload: RawPayload, key: string): number | undefined {
  const value = payload[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function eventString(payload: RawPayload, key: string): string | undefined {
  const value = payload[key];
  return typeof value === "string" ? value : undefined;
}

export interface LaunchProgress {
  /** 已下载字节 */
  downloaded: number;
  /** 总字节（可能为 0，表示未知） */
  total: number;
  /** 当前文件类别 / 阶段描述 */
  stage: string;
}

export interface LaunchDiagnostic {
  category: string;
  title: string;
  summary?: string;
  message: string;
}

export interface LauncherState {
  running: boolean;
  starting: boolean;
  cancelRequested: boolean;
  exitCode: number | null;
  reportTarget: Pick<LaunchArgs, "versionId" | "gameDir"> | null;
  progress: LaunchProgress | null;
  percent: number | null;
  speedBps: number | null;
  stage: string;
  error: string;
  warning: string;
  diagnostic: LaunchDiagnostic | null;
}

export interface LaunchArgs {
  versionId: string;
  gameDir: string;
  username: string;
  memoryMb: number;
  source: SourcePolicy;
}

export interface UseLauncher extends LauncherState {
  launch: (args: LaunchArgs) => Promise<void>;
  cancel: () => void;
  clearError: () => void;
  openLogs: (args: Pick<LaunchArgs, "versionId" | "gameDir">) => Promise<void>;
  exportErrorReport: (
    args: Pick<LaunchArgs, "versionId" | "gameDir">
  ) => Promise<string | null>;
}

/**
 * 启动流程状态机：封装 `launch_game` 调用 + `launcher-event` 订阅。
 *
 * 界面只消费准备阶段的进度与失败信息：运行日志由后端落盘，
 * 前端不再保留第二份日志缓冲，避免出现与磁盘不一致的重复记录。
 * 只处理当前线格式中有启动身份的事件，缺字段不崩溃。
 */
function useLaunchController(): UseLauncher {
  const [state, setState] = useState<Omit<LauncherState, "percent">>({
    running: false,
    starting: false,
    cancelRequested: false,
    exitCode: null,
    reportTarget: null,
    progress: null,
    speedBps: null,
    stage: "",
    error: "",
    warning: "",
    diagnostic: null,
  });
  const mounted = useRef(true);
  const subscription = useRef<Promise<boolean> | null>(null);
  // Synchronous ownership prevents duplicate calls before disabled controls commit.
  const lifecycle = useRef({
    active: false,
    preparing: false,
    cancelling: false,
    issued: false,
    generation: 0,
    launchId: "",
  });

  useEffect(() => {
    mounted.current = true;
    if (!isTauri())
      return () => {
        mounted.current = false;
      };
    let disposed = false;
    const unlistenPromise = listen<RawPayload>("launcher-event", (event) => {
      if (disposed) return;
      const payload = (event?.payload ?? {}) as RawPayload;
      const kind = eventString(payload, "type");
      const launchId = eventString(payload, "launchId");
      if (!launchId) return;
      if (lifecycle.current.launchId && lifecycle.current.launchId !== launchId)
        return;
      if (!lifecycle.current.launchId) {
        if (kind !== "preparing" && kind !== "started") return;
        lifecycle.current.launchId = launchId;
        lifecycle.current.active = true;
        lifecycle.current.preparing = true;
        lifecycle.current.issued = true;
      }

      switch (kind) {
        case "preparing": {
          const versionId = eventString(payload, "versionId");
          const gameDir = eventString(payload, "gameDir");
          lifecycle.current.active = true;
          lifecycle.current.preparing = true;
          lifecycle.current.cancelling = false;
          setState((prev) => ({
            ...prev,
            running: true,
            starting: true,
            cancelRequested: false,
            error: "",
            warning: "",
            diagnostic: null,
            progress: null,
            speedBps: null,
            exitCode: null,
            stage: "准备中...",
            reportTarget:
              versionId && gameDir ? { versionId, gameDir } : prev.reportTarget,
          }));
          break;
        }
        case "progress": {
          if (!lifecycle.current.preparing) break;
          const downloaded =
            eventNumber(payload, "downloaded") ?? 0;
          const total =
            eventNumber(payload, "total") ?? 0;
          const stage =
            eventString(payload, "fileType") ?? "";
          setState((prev) =>
            prev.starting
              ? { ...prev, progress: { downloaded, total, stage } }
              : prev
          );
          break;
        }
        case "speed": {
          if (!lifecycle.current.preparing) break;
          const bps =
            eventNumber(payload, "bps") ?? null;
          setState((prev) =>
            prev.starting ? { ...prev, speedBps: bps } : prev
          );
          break;
        }
        case "stage": {
          if (!lifecycle.current.preparing) break;
          const message =
            eventString(payload, "message") ?? "";
          setState((prev) =>
            prev.starting ? { ...prev, stage: message } : prev
          );
          break;
        }
        case "started": {
          if (!lifecycle.current.active) break;
          lifecycle.current.preparing = false;
          lifecycle.current.cancelling = false;
          setState((prev) => ({
            ...prev,
            running: true,
            starting: false,
            cancelRequested: false,
            stage: "",
            progress: null,
            speedBps: null,
          }));
          break;
        }
        case "cancelled": {
          if (!lifecycle.current.preparing) break;
          lifecycle.current.active = false;
          lifecycle.current.preparing = false;
          lifecycle.current.cancelling = false;
          lifecycle.current.issued = false;
          setState((prev) => ({
            ...prev,
            running: false,
            starting: false,
            cancelRequested: false,
            exitCode: null,
            progress: null,
            speedBps: null,
            stage: "",
            error: "",
            diagnostic: null,
          }));
          break;
        }
        case "close": {
          lifecycle.current.active = false;
          lifecycle.current.preparing = false;
          lifecycle.current.cancelling = false;
          lifecycle.current.issued = false;
          const code = eventNumber(payload, "code");
          setState((prev) => ({
            ...prev,
            running: false,
            starting: false,
            cancelRequested: false,
            exitCode: code ?? null,
            progress: null,
            speedBps: null,
            stage: "",
          }));
          break;
        }
        case "diagnostic": {
          const title = eventString(payload, "title") ?? "启动诊断";
          const message =
            eventString(payload, "message") ??
            "未能确定具体原因。";
          const summary = eventString(payload, "summary") ?? title;
          const category =
            eventString(payload, "category") ?? "unknown";
          setState((prev) => ({
            ...prev,
            diagnostic: { category, title, summary, message },
          }));
          break;
        }
        case "warning": {
          const message =
            eventString(payload, "message") ??
            "启动项未生效";
          setState((prev) => ({ ...prev, warning: message }));
          break;
        }
        case "error": {
          lifecycle.current.active = false;
          lifecycle.current.preparing = false;
          lifecycle.current.cancelling = false;
          lifecycle.current.issued = false;
          const message =
            eventString(payload, "message") ?? "未知错误";
          setState((prev) => ({
            ...prev,
            error: message,
            running: false,
            starting: false,
            cancelRequested: false,
            progress: null,
            speedBps: null,
            stage: "",
          }));
          break;
        }
        default:
          break;
      }
    });
    const ready = unlistenPromise.then(
      () => !disposed,
      () => false
    );
    subscription.current = ready;
    return () => {
      disposed = true;
      mounted.current = false;
      if (subscription.current === ready) subscription.current = null;
      void unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  const launch = useCallback(async (args: LaunchArgs) => {
    if (!mounted.current || lifecycle.current.active) return;
    const generation = ++lifecycle.current.generation;
    const launchId = crypto.randomUUID();
    lifecycle.current.launchId = launchId;
    lifecycle.current.active = true;
    lifecycle.current.preparing = true;
    lifecycle.current.cancelling = false;
    lifecycle.current.issued = false;
    setState((prev) => ({
      ...prev,
      error: "",
      warning: "",
      diagnostic: null,
      progress: null,
      speedBps: null,
      exitCode: null,
      reportTarget: { versionId: args.versionId, gameDir: args.gameDir },
      stage: "准备中...",
      running: true,
      starting: true,
      cancelRequested: false,
    }));
    try {
      const listener = subscription.current;
      if (!listener || !(await listener))
        throw new Error("启动状态订阅未能建立，请稍后重试或重新打开启动器。");
      if (!mounted.current || lifecycle.current.generation !== generation)
        return;
      if (lifecycle.current.cancelling) {
        lifecycle.current.active = false;
        lifecycle.current.preparing = false;
        lifecycle.current.cancelling = false;
        setState((prev) => ({
          ...prev,
          running: false,
          starting: false,
          cancelRequested: false,
          stage: "",
        }));
        return;
      }
      lifecycle.current.issued = true;
      await invoke("launch_game", {
        launchId,
        versionId: args.versionId,
        gameDir: args.gameDir,
        username: args.username,
        memoryMb: args.memoryMb,
        source: args.source,
      });
    } catch (error) {
      if (!mounted.current || lifecycle.current.generation !== generation)
        return;
      lifecycle.current.active = false;
      lifecycle.current.preparing = false;
      lifecycle.current.cancelling = false;
      lifecycle.current.issued = false;
      const message = error instanceof Error ? error.message : String(error);
      setState((prev) => ({
        ...prev,
        error: message,
        running: false,
        starting: false,
        cancelRequested: false,
        progress: null,
        speedBps: null,
        stage: "",
      }));
    }
  }, []);

  const cancel = useCallback(() => {
    if (
      !isTauri() ||
      !lifecycle.current.preparing ||
      lifecycle.current.cancelling
    )
      return;
    const generation = lifecycle.current.generation;
    lifecycle.current.cancelling = true;
    setState((prev) => ({ ...prev, cancelRequested: true }));
    if (!lifecycle.current.issued) return;
    void invoke("launch_cancel", {
      launchId: lifecycle.current.launchId,
    }).catch(() => {
      if (
        !mounted.current ||
        lifecycle.current.generation !== generation ||
        !lifecycle.current.preparing
      )
        return;
      lifecycle.current.cancelling = false;
      setState((prev) => ({ ...prev, cancelRequested: false }));
    });
  }, []);

  const clearError = useCallback(() => {
    setState((prev) => ({ ...prev, error: "", diagnostic: null }));
  }, []);

  const openLogs = useCallback(
    async (args: Pick<LaunchArgs, "versionId" | "gameDir">) => {
      try {
        await invoke("launch_log_open", {
          dir: args.gameDir,
          versionId: args.versionId,
        });
      } catch {
        // Opening a directory must not replace the original failure diagnostic.
      }
    },
    []
  );

  const exportErrorReport = useCallback(
    (args: Pick<LaunchArgs, "versionId" | "gameDir">) =>
      invoke<string | null>("launch_error_report_export", {
        dir: args.gameDir,
        versionId: args.versionId,
        failure: [
          state.diagnostic?.summary,
          state.diagnostic?.message,
          state.error,
        ]
          .filter(Boolean)
          .join("\n")
          .slice(0, 8000),
      }),
    [state.diagnostic, state.error]
  );

  const percent = useMemo<number | null>(() => {
    if (!state.progress || state.progress.total <= 0) return null;
    return Math.max(
      0,
      Math.min(
        100,
        Math.round((state.progress.downloaded / state.progress.total) * 100)
      )
    );
  }, [state.progress]);

  return {
    ...state,
    percent,
    launch,
    cancel,
    clearError,
    openLogs,
    exportErrorReport,
  };
}

const LauncherContext = createContext<UseLauncher | null>(null);

// Own the subscription above keyed routes so navigation cannot abandon a launch.
export function LauncherProvider({ children }: { children: ReactNode }) {
  const launcher = useLaunchController();
  return createElement(LauncherContext.Provider, { value: launcher }, children);
}

export function useLauncher(): UseLauncher {
  const launcher = useContext(LauncherContext);
  if (!launcher) throw new Error("useLauncher requires LauncherProvider");
  return launcher;
}
