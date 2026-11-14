/** 成功与取消不保留历史，失败卡等待手动移除；等待项在后端启动前没有 taskId。 */
import { useSyncExternalStore } from "react";
import {
  cancelInstall,
  installVersion,
  saveLoaderInstallerManaged,
  subscribeInstallEventsReady,
  listTaskSnapshots,
  type InstallArgs,
} from "./ipc";
import { describeInstallError } from "./parse";
import { resolvePercent } from "./format";
import type {
  DownloadCenterState,
  DownloadOverview,
  DownloadState,
  DownloadTask,
  InstallEvent,
  TaskSnapshot,
} from "./types";
import { setLaunchInstance } from "../../lib/instances";
import { isTauri } from "../../lib/settings";
import { showToast } from "../../lib/toast";

const BUBBLE_DONE_LINGER_MS = 5000;
const SNAPSHOT_INTERVAL_MS = 500;
const QUEUE_LIMIT = 32;
const FILE_HISTORY_LIMIT = 128;
const INITIAL: DownloadState = {
  status: "idle",
  task: null,
  stage: "",
  detail: "",
  percent: null,
  filesDone: 0,
  filesTotal: 0,
  speedBps: null,
  remainingMs: null,
  startedAt: null,
  endedAt: null,
  files: [],
  message: "",
  bubbleDismissed: false,
  target: "",
  cancelRequested: false,
  transferredBytes: null,
  completedBytes: null,
  totalBytes: null,
  stages: [],
};
let center: DownloadCenterState = {
  tasks: [],
  queue: [],
  selectedTaskId: null,
  entrance: 0,
  bubbleDismissed: false,
  error: "",
  startError: "",
};
let selected = INITIAL;
const listeners = new Set<() => void>();
const dismissed = new Set<string>();
const revisions = new Map<string, number>();
const eventVersions = new Map<string, number>();
const earlyEvents = new Map<string, InstallEvent[]>();
const terminalEffects = new Set<string>();
let attachment: Promise<void> | null = null;
let refreshing: Promise<void> | null = null;
let hideTimer: number | null = null;
let eventSequence = 0;

type QueueRequest = {
  start: () => Promise<string | null>;
  resolve: (id: string | null) => void;
  reject: (error: unknown) => void;
};
const queueRequests = new Map<string, QueueRequest>();
let serialStarting = false;
let queueSequence = 0;

function isSerial(task: Omit<DownloadTask, "taskId"> | null): boolean {
  return task?.kind === "version" || task?.kind === "modpack";
}

async function drainQueue(): Promise<void> {
  if (
    serialStarting ||
    center.tasks.some(
      (item) => item.status === "running" && isSerial(item.task)
    )
  )
    return;
  const queued = center.queue[0];
  if (!queued) return;
  const request = queueRequests.get(queued.queueId);
  if (!request) return;
  serialStarting = true;
  publish({
    queue: center.queue.map((item) =>
      item.queueId === queued.queueId ? { ...item, starting: true } : item
    ),
  });
  try {
    request.resolve(
      await startManagedDownloadInternal(queued.task, request.start)
    );
  } catch (error) {
    request.reject(error);
  } finally {
    queueRequests.delete(queued.queueId);
    publish({
      queue: center.queue.filter((item) => item.queueId !== queued.queueId),
    });
    serialStarting = false;
    void drainQueue();
    settleBubble();
  }
}

/** 等待项只保留闭包，取消不会调用后端，也不会创建实例目录。 */
export function cancelQueuedDownload(queueId: string): void {
  const queued = center.queue.find((item) => item.queueId === queueId);
  if (!queued || queued.starting) return;
  queueRequests.get(queueId)?.resolve(null);
  queueRequests.delete(queueId);
  publish({ queue: center.queue.filter((item) => item.queueId !== queueId) });
  void drainQueue();
  settleBubble();
}

export function isDownloadBubbleVisible(value: DownloadCenterState): boolean {
  const pending =
    value.queue.length > 0 ||
    value.tasks.some((item) => item.status === "running");
  return (
    pending ||
    (!value.bubbleDismissed && (value.tasks.length > 0 || !!value.startError))
  );
}

/** 聚合当前阶段快照，未知进度按零参与均值，不推测未来阶段。 */
export function downloadOverview(value: DownloadCenterState): DownloadOverview {
  const active = value.tasks.filter((item) => item.status === "running");
  if (active.length === 0)
    return { percent: 100, speedBps: 0, remainingFiles: 0 };
  let percentSum = 0;
  let speedBps = 0;
  let remainingFiles = 0;
  for (const item of active) {
    percentSum +=
      resolvePercent(item.percent, item.filesDone, item.filesTotal) ?? 0;
    if (
      item.speedBps !== null &&
      Number.isFinite(item.speedBps) &&
      item.speedBps > 0
    )
      speedBps += item.speedBps;
    remainingFiles += Math.max(0, item.filesTotal - item.filesDone);
  }
  return { percent: percentSum / active.length, speedBps, remainingFiles };
}

function publish(next: Partial<DownloadCenterState>): void {
  const wasVisible = isDownloadBubbleVisible(center);
  center = { ...center, ...next };
  // 移除成功与取消终态时同时屏蔽旧快照，防止任务再次进入列表。
  const terminal = center.tasks.filter(
    (item) => item.status === "done" || item.status === "cancelled"
  );
  if (terminal.length > 0) {
    for (const item of terminal) {
      if (item.task) dismissed.add(item.task.taskId);
    }
    center.tasks = center.tasks.filter(
      (item) => item.status !== "done" && item.status !== "cancelled"
    );
  }
  const active = center.tasks.filter((item) => item.status === "running");
  // 刚完成的任务虽移出列表，仍把终态快照留给 selected：社区详情页依赖
  // useDownloadState().endedAt 的变化重读实例，直接归零会丢掉这次刷新信号。
  const terminalSelected = terminal.find(
    (item) => item.task?.taskId === center.selectedTaskId
  );
  const keptTerminal =
    selected.task?.taskId === center.selectedTaskId &&
    (selected.status === "done" || selected.status === "cancelled")
      ? selected
      : null;
  selected =
    center.tasks.find((item) => item.task?.taskId === center.selectedTaskId) ??
    terminalSelected ??
    keptTerminal ??
    active[0] ??
    center.tasks[center.tasks.length - 1] ??
    INITIAL;
  center.selectedTaskId = selected.task?.taskId ?? null;
  if (!wasVisible && isDownloadBubbleVisible(center)) center.entrance += 1;
  for (const listener of listeners) listener();
}

function clearHideTimer(): void {
  if (hideTimer !== null) window.clearTimeout(hideTimer);
  hideTimer = null;
}

function settleBubble(): void {
  if (
    center.queue.length > 0 ||
    center.tasks.some((item) => item.status === "running")
  )
    return;
  clearHideTimer();
  if (center.startError || center.tasks.some((item) => item.status === "error"))
    return;
  hideTimer = window.setTimeout(() => {
    hideTimer = null;
    publish({ bubbleDismissed: true });
  }, BUBBLE_DONE_LINGER_MS);
}

function patchTask(taskId: string, next: Partial<DownloadState>): void {
  publish({
    tasks: center.tasks.map((item) =>
      item.task?.taskId === taskId ? { ...item, ...next } : item
    ),
  });
}

function addTask(item: DownloadState, focus: boolean): void {
  const taskId = item.task?.taskId;
  if (!taskId) return;
  clearHideTimer();
  publish({
    tasks: [...center.tasks, item],
    selectedTaskId:
      focus || !center.selectedTaskId ? taskId : center.selectedTaskId,
    bubbleDismissed: false,
  });
}

function runTerminalEffects(
  event: Extract<InstallEvent, { kind: "done" }>,
  item: DownloadState
): void {
  if (terminalEffects.has(event.taskId)) return;
  terminalEffects.add(event.taskId);
  if (event.ok && item.task?.kind !== "installer") {
    window.dispatchEvent(new CustomEvent("nowhere:instances-changed"));
  }
  if (event.ok && event.result) {
    setLaunchInstance({
      root: event.result.root,
      versionId: event.result.versionId,
      displayName: event.result.displayName,
      loader: "unknown",
      icon: "grass",
    });
  }
  const label = item.task?.instanceName || item.task?.versionId || "下载任务";
  if (event.cancelled) showToast(`${label} 已取消`, "info");
  else if (event.ok) showToast(`${label} 下载完成`, "success");
  else showToast(`${label} 下载失败`, "error");
}

function handleEvent(event: InstallEvent): void {
  if (dismissed.has(event.taskId)) return;
  eventVersions.set(event.taskId, ++eventSequence);
  const item = center.tasks.find((item) => item.task?.taskId === event.taskId);
  if (!item) {
    const events = earlyEvents.get(event.taskId) ?? [];
    earlyEvents.set(event.taskId, [
      ...events.slice(-(FILE_HISTORY_LIMIT - 1)),
      event,
    ]);
    if (earlyEvents.size > QUEUE_LIMIT) {
      const oldest = earlyEvents.keys().next().value;
      if (oldest) earlyEvents.delete(oldest);
    }
    void refreshDownloads();
    return;
  }
  if (event.kind === "done") {
    runTerminalEffects(event, item);
    if (item.status === "running") {
      patchTask(event.taskId, {
        status: event.cancelled ? "cancelled" : event.ok ? "done" : "error",
        cancelRequested: event.cancelled,
        message: event.message,
        endedAt: Date.now(),
        percent: event.ok ? 100 : item.percent,
        speedBps: null,
        remainingMs: null,
      });
      settleBubble();
    }
    void drainQueue();
    void refreshDownloads();
    return;
  }
  if (item.status !== "running") return;
  switch (event.kind) {
    case "stage": {
      const changed = item.stage !== event.stage;
      patchTask(event.taskId, {
        stage: event.stage,
        detail: event.detail,
        ...(changed
          ? {
              percent: null,
              filesDone: 0,
              filesTotal: 0,
              completedBytes: null,
              totalBytes: null,
              remainingMs: null,
              stages: [
                ...item.stages,
                { name: event.stage, detail: event.detail },
              ].slice(-FILE_HISTORY_LIMIT),
            }
          : {}),
      });
      break;
    }
    case "progress":
      patchTask(event.taskId, {
        percent:
          item.totalBytes === null
            ? event.filesTotal > 0
              ? event.percent
              : null
            : item.percent,
        filesDone: event.filesDone,
        filesTotal: event.filesTotal,
        remainingMs:
          event.remainingMs && event.remainingMs > 0 ? event.remainingMs : null,
      });
      break;
    case "file": {
      const file = {
        name: event.name,
        percent: event.percent,
        done: event.done,
      };
      const exists = item.files.some((entry) => entry.name === event.name);
      patchTask(event.taskId, {
        files: exists
          ? item.files.map((entry) =>
              entry.name === event.name ? file : entry
            )
          : [...item.files, file].slice(-FILE_HISTORY_LIMIT),
      });
      break;
    }
    case "error":
      patchTask(event.taskId, { message: event.message });
      break;
  }
}

function applySnapshot(snapshot: TaskSnapshot, requestedAt: number): void {
  if (dismissed.has(snapshot.taskId)) return;
  const item = center.tasks.find(
    (item) => item.task?.taskId === snapshot.taskId
  );
  if ((revisions.get(snapshot.taskId) ?? -1) > snapshot.revision) return;
  if (item && item.status !== "running" && snapshot.status !== item.status)
    return;
  revisions.set(snapshot.taskId, snapshot.revision);
  // 阶段事件不能被在途旧快照覆盖；真实字节仍需采样，避免高频事件使速度永远未知。
  if (
    item &&
    snapshot.status === "running" &&
    (eventVersions.get(snapshot.taskId) ?? 0) > requestedAt
  ) {
    patchTask(snapshot.taskId, {
      transferredBytes: snapshot.transferredBytes,
      speedBps: snapshot.speedBps,
      cancelRequested: snapshot.cancelRequested || item.cancelRequested,
    });
    return;
  }
  const { taskId, kind, revision: _revision, label, ...progress } = snapshot;
  const task: DownloadTask = item?.task
    ? { ...item.task, kind: kind ?? item.task.kind }
    : {
        taskId,
        kind,
        versionId: label,
        instanceName: label,
        dir: snapshot.target,
        loader: "",
        loaderVersion: null,
        source: "official_auto",
      };
  const next: DownloadState = {
    ...INITIAL,
    ...progress,
    task,
    cancelRequested:
      snapshot.status === "running"
        ? snapshot.cancelRequested || !!item?.cancelRequested
        : snapshot.cancelRequested,
  };
  if (snapshot.status === "done" || snapshot.status === "cancelled") {
    // 无历史：成功/取消的终态快照不进入列表。已知任务先落终态（publish 立即移除并屏蔽），
    // 缓冲的 done 事件仍要触发实例刷新 / 启动实例副作用，必须在移除前消费。
    const buffered = earlyEvents.get(taskId);
    if (buffered) {
      earlyEvents.delete(taskId);
      for (const event of buffered) {
        if (event.kind === "done") runTerminalEffects(event, item ?? next);
      }
    }
    if (item) {
      patchTask(taskId, next);
      settleBubble();
    }
    return;
  }
  if (item) patchTask(taskId, next);
  else addTask(next, false);
  if (snapshot.status === "error" && (!item || item.status === "running"))
    settleBubble();
  const buffered = earlyEvents.get(taskId);
  if (buffered) {
    earlyEvents.delete(taskId);
    buffered.forEach(handleEvent);
  }
}

export function refreshDownloads(): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  if (refreshing) return refreshing;
  const requestedAt = eventSequence;
  refreshing = listTaskSnapshots()
    .then((snapshots) => {
      for (const snapshot of snapshots) applySnapshot(snapshot, requestedAt);
      if (center.error) publish({ error: "" });
      void drainQueue();
    })
    .catch((error: unknown) => {
      publish({ error: `下载状态同步失败：${describeInstallError(error)}` });
    })
    .finally(() => {
      refreshing = null;
    });
  return refreshing;
}

function ensureAttached(): Promise<void> {
  if (!attachment) {
    attachment = subscribeInstallEventsReady(handleEvent)
      .then(() => {
        void refreshDownloads();
        window.setInterval(() => void refreshDownloads(), SNAPSHOT_INTERVAL_MS);
      })
      .catch((error: unknown) => {
        attachment = null;
        throw error;
      });
  }
  return attachment;
}

export async function connectDownloadCenter(): Promise<void> {
  if (!isTauri()) return;
  try {
    await ensureAttached();
  } catch (error) {
    publish({ error: `下载事件连接失败：${describeInstallError(error)}` });
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

async function startManagedDownloadInternal(
  task: Omit<DownloadTask, "taskId">,
  start: () => Promise<string | null>
): Promise<string | null> {
  await ensureAttached();
  const taskId = await start();
  if (taskId === null) return null;
  if (!taskId) throw new Error("下载任务未启动，请重试");
  const existing = center.tasks.find((item) => item.task?.taskId === taskId);
  if (existing) {
    patchTask(taskId, { task: { ...task, taskId } });
    selectDownload(taskId);
  } else {
    addTask(
      {
        ...INITIAL,
        status: "running",
        task: { ...task, taskId },
        stage: "准备下载",
        startedAt: Date.now(),
      },
      true
    );
  }
  const buffered = earlyEvents.get(taskId) ?? [];
  earlyEvents.delete(taskId);
  buffered.forEach(handleEvent);
  void refreshDownloads();
  return taskId;
}

/** version/modpack 串行，普通资源与安装器并行；取消对话框/等待项均返回 null。 */
export async function startManagedDownload(
  task: Omit<DownloadTask, "taskId">,
  start: () => Promise<string | null>
): Promise<string | null> {
  // 旧本地安装调用仍默认串行；恢复的未知快照不能被推断成版本安装。
  const requestTask = { ...task, kind: task.kind ?? "version" };
  try {
    if (!isSerial(requestTask))
      return await startManagedDownloadInternal(requestTask, start);
    await ensureAttached();
    await refreshDownloads();
    if (center.error) throw new Error(center.error);
    if (center.queue.length >= QUEUE_LIMIT)
      throw new Error("等待队列已满，请等待任务开始或取消部分等待项。");
    const queueId = `queue-${++queueSequence}`;
    const result = new Promise<string | null>((resolve, reject) => {
      queueRequests.set(queueId, { start, resolve, reject });
    });
    clearHideTimer();
    publish({
      queue: [
        ...center.queue,
        { queueId, task: requestTask, enqueuedAt: Date.now(), starting: false },
      ],
      bubbleDismissed: false,
    });
    void drainQueue();
    return await result;
  } catch (error) {
    clearHideTimer();
    publish({
      startError: `${
        task.instanceName || task.versionId
      }：${describeInstallError(error)}`,
      bubbleDismissed: false,
    });
    throw error;
  }
}

/** null 不创建任务、不改变当前选择、不触发浮窗入场。 */
export async function startInstallerDownload(
  args: import("./types").InstallerDownloadArgs
): Promise<string | null> {
  return startManagedDownload(
    {
      versionId: args.mcVersion,
      instanceName: `${args.loader} ${args.loaderVersion} 安装器`,
      dir: "",
      loader: args.loader,
      loaderVersion: args.loaderVersion,
      source: args.source,
      kind: "installer",
    },
    () => saveLoaderInstallerManaged(args)
  );
}

export async function startDownload(args: InstallArgs): Promise<void> {
  try {
    await startManagedDownload({ ...args, kind: "version" }, () =>
      installVersion(args)
    );
  } catch {
    /* 错误由任务中心显示，保留旧入口的 Promise<void> 契约。 */
  }
}

export async function cancelDownload(
  taskId = selected.task?.taskId
): Promise<void> {
  const item = center.tasks.find((item) => item.task?.taskId === taskId);
  if (!taskId || !item || item.status !== "running" || item.cancelRequested)
    return;
  patchTask(taskId, {
    cancelRequested: true,
    message: "正在取消，等待下载与文件清理结束…",
  });
  try {
    await cancelInstall(taskId);
    await refreshDownloads();
  } catch (error) {
    patchTask(taskId, {
      cancelRequested: false,
      message: describeInstallError(error),
    });
  }
}

export function selectDownload(taskId: string): void {
  if (center.tasks.some((item) => item.task?.taskId === taskId))
    publish({ selectedTaskId: taskId });
}

export function dismissBubble(): void {
  if (
    center.queue.length > 0 ||
    center.tasks.some((item) => item.status === "running")
  )
    return;
  clearHideTimer();
  publish({ bubbleDismissed: true });
}

/** 一键清理保留的失败记录与启动错误；运行中和取消收尾中的任务始终保留。 */
export function clearDownload(): void {
  for (const item of center.tasks) {
    if (item.status !== "running" && item.task) dismissed.add(item.task.taskId);
  }
  const tasks = center.tasks.filter((item) => item.status === "running");
  publish({
    tasks,
    selectedTaskId: tasks[0]?.task?.taskId ?? null,
    startError: "",
  });
  settleBubble();
}

/** 屏蔽已移除任务的后续快照，运行中任务不可移除。 */
export function dismissDownload(taskId: string): void {
  const item = center.tasks.find((entry) => entry.task?.taskId === taskId);
  if (!item || item.status === "running") return;
  dismissed.add(taskId);
  const tasks = center.tasks.filter((entry) => entry.task?.taskId !== taskId);
  publish({
    tasks,
    selectedTaskId:
      center.selectedTaskId === taskId
        ? tasks.find((entry) => entry.status === "running")?.task?.taskId ??
          tasks[0]?.task?.taskId ??
          null
        : center.selectedTaskId,
  });
  settleBubble();
}

export function clearDownloadStartError(): void {
  publish({ startError: "" });
  settleBubble();
}

export function useDownloadCenter(): DownloadCenterState {
  return useSyncExternalStore(
    subscribe,
    () => center,
    () => center
  );
}

export function useDownloadState(): DownloadState {
  return useSyncExternalStore(
    subscribe,
    () => selected,
    () => selected
  );
}
