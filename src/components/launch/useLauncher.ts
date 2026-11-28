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

type RawPayload = Record<string, unknown>;

function eventNumber(payload: RawPayload, key: string): number | undefined {
  const value = payload[key];
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function eventString(payload: RawPayload, key: string): string | undefined {
  const value = payload[key];
  return typeof value === "string" ? value : undefined;
}

export type LaunchPhase = "preparing" | "checking" | "downloading";

export interface LaunchProgress {
  /** Completed files, not transferred bytes. */
  downloaded: number;
  /** Total files; zero means unknown. */
  total: number;
  stage: string;
}

export interface LaunchDiagnostic {
  category: string;
  title: string;
  summary?: string;
  message: string;
}

type LaunchTarget = Pick<LaunchArgs, "versionId" | "gameDir" | "displayName">;

export interface LauncherState {
  running: boolean;
  starting: boolean;
  cancelRequested: boolean;
  exitCode: number | null;
  activeTarget: LaunchTarget | null;
  reportTarget: LaunchTarget | null;
  failureId: string | null;
  phase: LaunchPhase;
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
  displayName?: string;
  username: string;
  memoryMb: number;
  source: SourcePolicy;
}

export interface UseLauncher extends LauncherState {
  launch: (args: LaunchArgs) => Promise<void>;
  cancel: () => void;
  clearError: () => void;
  openLogs: (args: LaunchTarget) => Promise<void>;
  exportErrorReport: (args: LaunchTarget) => Promise<string | null>;
}

interface LaunchFailure {
  launchId: string;
  target: LaunchTarget | null;
  error: string;
  diagnostic: LaunchDiagnostic | null;
}

interface Session {
  target: LaunchTarget | null;
  closed: boolean;
}

type ControllerState = Omit<
  LauncherState,
  "percent" | "error" | "diagnostic" | "failureId"
> & {
  failures: LaunchFailure[];
};

const EMPTY_PROGRESS = {
  phase: "preparing" as const,
  progress: null,
  speedBps: null,
  stage: "",
};

function progressLabel(kind: string, checking: boolean): string {
  const category = checking ? kind.slice("校验:".length) : kind;
  const names: Record<string, string> = {
    assets: "游戏资源",
    libraries: "依赖库",
    java: "Java",
    client: "游戏主文件",
    minecraft: "游戏主文件",
  };
  return `${checking ? "正在校验" : "正在下载"}${
    names[category.toLowerCase()] || category || "游戏文件"
  }`;
}

function useLaunchController(): UseLauncher {
  const [state, setState] = useState<ControllerState>({
    running: false,
    starting: false,
    cancelRequested: false,
    exitCode: null,
    activeTarget: null,
    reportTarget: null,
    ...EMPTY_PROGRESS,
    warning: "",
    failures: [],
  });
  const mounted = useRef(true);
  const subscription = useRef<Promise<boolean> | null>(null);
  // Only preparation owns the primary button; spawned games keep independent identities.
  const sessions = useRef(new Map<string, Session>());
  const lifecycle = useRef({
    preparing: false,
    cancelling: false,
    issued: false,
    generation: 0,
    launchId: "",
  });

  const hasActiveSessions = () =>
    [...sessions.current.values()].some((session) => !session.closed);

  const recordFailure = useCallback((failure: LaunchFailure) => {
    setState((prev) => {
      const index = prev.failures.findIndex(
        (item) => item.launchId === failure.launchId
      );
      const failures = [...prev.failures];
      if (index < 0) failures.push(failure);
      else
        failures[index] = {
          ...failure,
          error: failure.error || failures[index].error,
          diagnostic: failure.diagnostic || failures[index].diagnostic,
        };
      return { ...prev, failures };
    });
  }, []);

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
      if (!lifecycle.current.launchId) {
        if (kind !== "preparing" && kind !== "started") return;
        lifecycle.current.launchId = launchId;
        lifecycle.current.preparing = true;
        lifecycle.current.issued = true;
        sessions.current.set(launchId, { target: null, closed: false });
      }
      const session = sessions.current.get(launchId);
      if (!session) return;
      const current = lifecycle.current.launchId === launchId;
      const preparing =
        current && lifecycle.current.preparing && !session.closed;
      const finishPreparation = () => {
        if (!current) return;
        lifecycle.current.preparing = false;
        lifecycle.current.cancelling = false;
        lifecycle.current.issued = false;
      };
      const settledState = () => ({
        starting: false,
        cancelRequested: false,
        activeTarget: null,
        ...EMPTY_PROGRESS,
      });

      switch (kind) {
        case "preparing": {
          if (!preparing) break;
          const versionId = eventString(payload, "versionId");
          const gameDir = eventString(payload, "gameDir");
          if (versionId && gameDir)
            session.target = { ...session.target, versionId, gameDir };
          setState((prev) => ({
            ...prev,
            running: true,
            starting: true,
            cancelRequested: lifecycle.current.cancelling,
            activeTarget: session.target,
            reportTarget: session.target,
            ...EMPTY_PROGRESS,
            exitCode: null,
            stage: "准备中...",
          }));
          break;
        }
        case "progress": {
          if (!preparing) break;
          const downloaded = Math.max(
            0,
            Math.floor(eventNumber(payload, "downloaded") ?? 0)
          );
          const total = Math.max(
            0,
            Math.floor(eventNumber(payload, "total") ?? 0)
          );
          const stage = eventString(payload, "fileType") ?? "";
          const checking = stage.startsWith("校验:");
          const phase = checking ? "checking" : "downloading";
          setState((prev) => ({
            ...prev,
            phase,
            stage: progressLabel(stage, checking),
            progress: { downloaded, total, stage },
            speedBps:
              phase === "downloading" && prev.phase === phase
                ? prev.speedBps
                : null,
          }));
          break;
        }
        case "speed": {
          if (!preparing) break;
          const bps = eventNumber(payload, "bps");
          if (bps === undefined || bps < 0) break;
          setState((prev) => ({
            ...prev,
            phase: "downloading",
            progress: prev.phase === "downloading" ? prev.progress : null,
            stage:
              prev.phase === "downloading" ? prev.stage : "正在下载游戏文件",
            speedBps: bps,
          }));
          break;
        }
        case "estimated": {
          if (!preparing) break;
          const seconds = eventNumber(payload, "seconds");
          if (seconds === undefined || seconds < 0) break;
          setState((prev) =>
            prev.phase === "downloading"
              ? { ...prev, stage: `正在下载，预计剩余 ${Math.ceil(seconds)}s` }
              : prev
          );
          break;
        }
        case "stage": {
          if (!preparing) break;
          setState((prev) => ({
            ...prev,
            ...EMPTY_PROGRESS,
            stage: eventString(payload, "message") || "准备中...",
          }));
          break;
        }
        case "started": {
          if (!preparing) break;
          finishPreparation();
          setState((prev) => ({ ...prev, ...settledState(), running: true }));
          break;
        }
        case "cancelled": {
          if (!preparing) break;
          session.closed = true;
          finishPreparation();
          setState((prev) => ({
            ...prev,
            ...settledState(),
            running: hasActiveSessions(),
            exitCode: null,
          }));
          sessions.current.delete(launchId);
          break;
        }
        case "close": {
          if (session.closed) break;
          session.closed = true;
          finishPreparation();
          const code = eventNumber(payload, "code") ?? null;
          setState((prev) => ({
            ...prev,
            ...(current ? settledState() : {}),
            running: hasActiveSessions(),
            exitCode: code,
          }));
          // Failed exits retain their target until the following diagnostic arrives.
          if (code === 0) sessions.current.delete(launchId);
          break;
        }
        case "diagnostic": {
          const title = eventString(payload, "title") ?? "启动诊断";
          recordFailure({
            launchId,
            target: session.target,
            error: "",
            diagnostic: {
              title,
              message: eventString(payload, "message") ?? "未能确定具体原因。",
              summary: eventString(payload, "summary") ?? title,
              category: eventString(payload, "category") ?? "unknown",
            },
          });
          if (session.closed) sessions.current.delete(launchId);
          break;
        }
        case "warning": {
          if (!session.closed)
            setState((prev) => ({
              ...prev,
              warning: eventString(payload, "message") ?? "启动项未生效",
            }));
          break;
        }
        case "error": {
          session.closed = true;
          finishPreparation();
          setState((prev) => ({
            ...prev,
            ...(current ? settledState() : {}),
            running: hasActiveSessions(),
          }));
          recordFailure({
            launchId,
            target: session.target,
            error: eventString(payload, "message") ?? "未知错误",
            diagnostic: null,
          });
          break;
        }
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
  }, [recordFailure]);

  const launch = useCallback(
    async (args: LaunchArgs) => {
      if (!mounted.current || lifecycle.current.preparing) return;
      const generation = ++lifecycle.current.generation;
      const launchId = crypto.randomUUID();
      const target = {
        versionId: args.versionId,
        gameDir: args.gameDir,
        displayName: args.displayName,
      };
      lifecycle.current = {
        launchId,
        generation,
        preparing: true,
        cancelling: false,
        issued: false,
      };
      sessions.current.set(launchId, { target, closed: false });
      setState((prev) => ({
        ...prev,
        ...EMPTY_PROGRESS,
        warning: "",
        exitCode: null,
        activeTarget: target,
        reportTarget: target,
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
          sessions.current.delete(launchId);
          lifecycle.current.preparing = false;
          lifecycle.current.cancelling = false;
          setState((prev) => ({
            ...prev,
            ...EMPTY_PROGRESS,
            activeTarget: null,
            running: hasActiveSessions(),
            starting: false,
            cancelRequested: false,
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
        sessions.current.delete(launchId);
        lifecycle.current.preparing = false;
        lifecycle.current.cancelling = false;
        lifecycle.current.issued = false;
        setState((prev) => ({
          ...prev,
          ...EMPTY_PROGRESS,
          activeTarget: null,
          running: hasActiveSessions(),
          starting: false,
          cancelRequested: false,
        }));
        recordFailure({
          launchId,
          target,
          error: error instanceof Error ? error.message : String(error),
          diagnostic: null,
        });
      }
    },
    [recordFailure]
  );

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

  // Background failures wait until preparation ends so they cannot cover its cancel control.
  const failure = state.starting ? undefined : state.failures[0];
  const error = failure?.error ?? "";
  const diagnostic = failure?.diagnostic ?? null;
  const failureId = failure?.launchId ?? null;
  const clearError = useCallback(() => {
    if (!failureId) return;
    if (sessions.current.get(failureId)?.closed)
      sessions.current.delete(failureId);
    setState((prev) => ({
      ...prev,
      failures: prev.failures.filter((item) => item.launchId !== failureId),
    }));
  }, [failureId]);

  const openLogs = useCallback(async (args: LaunchTarget) => {
    try {
      await invoke("launch_log_open", {
        dir: args.gameDir,
        versionId: args.versionId,
      });
    } catch {
      // Opening a directory must not replace the original failure diagnostic.
    }
  }, []);

  const exportErrorReport = useCallback(
    (args: LaunchTarget) =>
      invoke<string | null>("launch_error_report_export", {
        dir: args.gameDir,
        versionId: args.versionId,
        failure: [diagnostic?.summary, diagnostic?.message, error]
          .filter(Boolean)
          .join("\n")
          .slice(0, 8000),
      }),
    [diagnostic, error]
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

  const { failures: _failures, ...visibleState } = state;
  return {
    ...visibleState,
    error,
    diagnostic,
    reportTarget: failure?.target ?? state.reportTarget,
    failureId: failure?.launchId ?? null,
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
