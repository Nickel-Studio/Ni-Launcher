import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri, type SourcePolicy } from "../../lib/settings";

/** 后端 `launcher-event` 的原始 payload（字段做宽松兜底）。 */
type RawPayload = Record<string, unknown>;

/** 从 payload 中按候选键名取出第一个数字。 */
function pickNumber(payload: RawPayload, keys: string[]): number | undefined {
  for (const key of keys) {
    const v = payload[key];
    if (typeof v === "number" && Number.isFinite(v)) return v;
    if (typeof v === "string" && v.trim() !== "" && !Number.isNaN(Number(v))) {
      return Number(v);
    }
  }
  return undefined;
}

/** 从 payload 中按候选键名取出第一个字符串。 */
function pickString(payload: RawPayload, keys: string[]): string | undefined {
  for (const key of keys) {
    const v = payload[key];
    if (typeof v === "string") return v;
    if (typeof v === "number") return String(v);
  }
  return undefined;
}

/** 归一化事件类型：兼容 `kind` 与旧版 `type`，并做大小写兜底。 */
function eventKind(payload: RawPayload): string {
  const raw = pickString(payload, ["kind", "type", "event"]) ?? "";
  return raw.toLowerCase();
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
  message: string;
}

export interface LauncherState {
  running: boolean;
  /** 准备阶段（下载 / 校验 / 启动前处理）：可取消，启动面板可见。 */
  starting: boolean;
  /** 最近一次游戏进程的退出码；未启动或事件缺失时为 null。 */
  exitCode: number | null;
  progress: LaunchProgress | null;
  /** 百分比 0–100（总量未知时为 null） */
  percent: number | null;
  /** 下载速度（字节/秒，最新值） */
  speedBps: number | null;
  /** 阶段文案；进程结束保留最后阶段 */
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
  /** 取消准备阶段：下载 / 校验 / 加载器安装即刻中断。 */
  cancel: () => void;
  /** 关闭失败弹窗后清掉一次性错误与诊断。 */
  clearError: () => void;
  openLogs: (args: Pick<LaunchArgs, "versionId" | "gameDir">) => Promise<void>;
}

/**
 * 启动流程状态机：封装 `launch_game` 调用 + `launcher-event` 订阅。
 *
 * 界面只消费准备阶段的进度与失败信息：运行日志由后端落盘，
 * 前端不再保留第二份日志缓冲，避免出现与磁盘不一致的重复记录。
 * 所有字段解析均做兼容兜底，缺字段不崩溃。
 */
export function useLauncher(): UseLauncher {
  const [state, setState] = useState<Omit<LauncherState, "percent">>({
    running: false,
    starting: false,
    exitCode: null,
    progress: null,
    speedBps: null,
    stage: "",
    error: "",
    warning: "",
    diagnostic: null,
  });

  useEffect(() => {
    // 非 Tauri 环境（纯浏览器）没有 IPC；跳过事件订阅，避免未捕获异常。
    if (!isTauri()) return;
    let disposed = false;
    const unlistenPromise = listen<RawPayload>("launcher-event", (event) => {
      if (disposed) return;
      const payload = (event?.payload ?? {}) as RawPayload;
      const kind = eventKind(payload);

      switch (kind) {
        case "progress":
        case "download":
        case "downloading": {
          const downloaded =
            pickNumber(payload, [
              "downloaded",
              "done",
              "current",
              "bytes",
              "completed",
            ]) ?? 0;
          const total =
            pickNumber(payload, ["total", "size", "expected", "full"]) ?? 0;
          const stage =
            pickString(payload, [
              "kind2",
              "fileType",
              "stage",
              "name",
              "file",
            ]) ?? "";
          setState((prev) => ({
            ...prev,
            progress: { downloaded, total, stage },
          }));
          break;
        }
        case "speed":
        case "download-speed": {
          const bps =
            pickNumber(payload, [
              "bps",
              "bytesPerSecond",
              "bytesPerSec",
              "speed",
            ]) ?? null;
          setState((prev) => ({ ...prev, speedBps: bps }));
          break;
        }
        case "stage":
        case "status": {
          const message =
            pickString(payload, ["message", "stage", "text", "label"]) ?? "";
          setState((prev) => ({ ...prev, stage: message }));
          break;
        }
        case "started": {
          // 准备阶段结束，游戏进程已生成：收起启动面板，保留运行态。
          setState((prev) => ({
            ...prev,
            starting: false,
            stage: "",
            progress: null,
            speedBps: null,
          }));
          break;
        }
        case "cancelled":
        case "canceled": {
          setState((prev) => ({
            ...prev,
            running: false,
            starting: false,
            exitCode: null,
            progress: null,
            speedBps: null,
            stage: "",
            error: "",
            diagnostic: null,
          }));
          break;
        }
        case "exit":
        case "close":
        case "closed":
        case "finished": {
          const code = pickNumber(payload, ["code", "exitCode", "status"]);
          setState((prev) => ({
            ...prev,
            running: false,
            starting: false,
            exitCode: code ?? null,
            progress: null,
            speedBps: null,
          }));
          break;
        }
        case "diagnostic": {
          const title = pickString(payload, ["title", "label"]) ?? "启动诊断";
          const message =
            pickString(payload, ["message", "reason", "detail"]) ??
            "请打开日志目录查看完整记录";
          const category =
            pickString(payload, ["category", "kind2"]) ?? "unknown";
          setState((prev) => ({
            ...prev,
            diagnostic: { category, title, message },
          }));
          break;
        }
        case "warning":
        case "warn": {
          const message =
            pickString(payload, ["message", "reason", "detail"]) ??
            "启动项未生效";
          // 非致命能力失败不得结束运行态；游戏仍在启动或运行。
          setState((prev) => ({ ...prev, warning: message }));
          break;
        }
        case "error":
        case "failed": {
          const message =
            pickString(payload, ["message", "error", "reason"]) ?? "未知错误";
          setState((prev) => ({ ...prev, error: message, running: false }));
          break;
        }
        default:
          // 未知事件：静默忽略，绝不崩溃
          break;
      }
    });
    // 事件插件不可用时也不产生未处理的 Promise 拒绝。
    void unlistenPromise.catch(() => {});

    return () => {
      disposed = true;
      void unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  const launch = useCallback(async (args: LaunchArgs) => {
    setState((prev) => ({
      ...prev,
      error: "",
      warning: "",
      diagnostic: null,
      progress: null,
      speedBps: null,
      exitCode: null,
      stage: "准备中...",
      running: true,
      starting: true,
    }));
    try {
      await invoke("launch_game", {
        versionId: args.versionId,
        gameDir: args.gameDir,
        username: args.username,
        memoryMb: args.memoryMb,
        source: args.source,
      });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setState((prev) => ({
        ...prev,
        error: message,
        running: false,
        starting: false,
        stage: "",
      }));
    }
  }, []);

  const cancel = useCallback(() => {
    if (!isTauri()) return;
    void invoke("launch_cancel").catch(() => {
      /* 已结束的启动没有可取消对象：忽略 */
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
        // 打开目录属于次要动作：失败由后端日志记录，不弹启动失败弹窗。
      }
    },
    []
  );

  const percent = useMemo<number | null>(() => {
    if (!state.progress || !state.progress.total) return null;
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
  };
}
