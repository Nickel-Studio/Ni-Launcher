import type {
  DownloadFile,
  DownloadStage,
  DownloadTaskKind,
  TaskSnapshot,
} from "./types";

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function number(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function percent(value: unknown): number | null {
  const result = number(value);
  return result === null ? null : Math.min(100, result * 100);
}

function taskKind(value: unknown): DownloadTaskKind | undefined {
  switch (value) {
    case "version":
    case "modpack":
    case "resource":
    case "installer":
    case "export":
      return value;
    default:
      return undefined;
  }
}

/** 字段校验集中于 IPC 边界；未知字节/阶段总量保留 null，不换算出虚构进度。 */
export function parseTaskSnapshots(raw: unknown): TaskSnapshot[] {
  if (!Array.isArray(raw)) throw new Error("下载状态格式无法识别，请重试。");
  return raw.flatMap((value): TaskSnapshot[] => {
    const item = record(value);
    if (!item || !text(item.taskId)) return [];
    const status = item.status;
    if (
      status !== "running" &&
      status !== "done" &&
      status !== "error" &&
      status !== "cancelled"
    )
      return [];
    const files: DownloadFile[] = Array.isArray(item.files)
      ? item.files.flatMap((value) => {
          const file = record(value);
          return file && text(file.name)
            ? [
                {
                  name: text(file.name),
                  percent: percent(file.percent) ?? 0,
                  done: file.done === true,
                },
              ]
            : [];
        })
      : [];
    const stages: DownloadStage[] = Array.isArray(item.stages)
      ? item.stages.flatMap((value) => {
          const stage = record(value);
          return stage && text(stage.name)
            ? [{ name: text(stage.name), detail: text(stage.detail) }]
            : [];
        })
      : [];
    return [
      {
        taskId: text(item.taskId),
        kind: taskKind(item.kind),
        revision: number(item.revision) ?? 0,
        target: text(item.target),
        label: text(item.label),
        status,
        cancelRequested: item.cancelRequested === true,
        stage: text(item.stage),
        detail: text(item.detail),
        percent: percent(item.percent),
        filesDone: number(item.filesDone) ?? 0,
        filesTotal: number(item.filesTotal) ?? 0,
        transferredBytes: number(item.transferredBytes),
        completedBytes: number(item.completedBytes),
        totalBytes: number(item.totalBytes),
        speedBps: number(item.speedBps),
        remainingMs: number(item.remainingMs),
        startedAt: number(item.startedAt) ?? 0,
        endedAt: number(item.endedAt),
        message: text(item.message),
        files,
        stages,
      },
    ];
  });
}
