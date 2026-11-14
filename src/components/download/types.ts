/**
 * 下载中心 · 数据模型。
 *
 * 与后端契约一一对应（命令名 / 事件名 / 字段一律 camelCase）：
 * - `list_remote_versions` → `{"latest":{release,snapshot},"versions":[{id,type,releaseTime,url}]}`
 * - `list_loaders`         → `{"forge":[..],"neoforge":[..],"fabric":[..],"liteloader":[..],"optifine":[..],"notes":{..},"providers":{..}}`
 * - `install_version`      → taskId
 * - `install-event`        → stage / progress / file / done / error
 */

import type { SourcePolicy } from "../../lib/settings";
/** source 的旧值仍作为兼容入口接受。 */
export type DownloadSource = SourcePolicy | "official" | "bmclapi";

/** `list_remote_versions` 归一化后的单条版本。 */
export interface RemoteVersion {
  id: string;
  /** release / snapshot / old_beta / old_alpha / unknown（后端原样透传）。 */
  type: string;
  /** RFC3339 时间字符串；后端未提供时为空串。 */
  releaseTime: string;
}

export interface RemoteManifest {
  /** 最新正式版 id；后端未提供时为空串。 */
  latestRelease: string;
  /** 最新预览版 id；后端未提供时为空串。 */
  latestSnapshot: string;
  /** 全部版本（默认保持后端的时间倒序）。 */
  versions: RemoteVersion[];
}

/** 加载器标识；五行固定展示顺序。 */
export type LoaderId =
  | "forge"
  | "neoforge"
  | "fabric"
  | "liteloader"
  | "optifine";

/** 加载器展示顺序（Forge / NeoForge / Fabric / LiteLoader / OptiFine）。 */
export const LOADER_IDS: readonly LoaderId[] = [
  "forge",
  "neoforge",
  "fabric",
  "liteloader",
  "optifine",
];

/** 加载器标识 → 界面标签。 */
export const LOADER_LABELS: Record<LoaderId, string> = {
  forge: "Forge",
  neoforge: "NeoForge",
  fabric: "Fabric",
  liteloader: "LiteLoader",
  optifine: "OptiFine",
};

export type LoaderProviderState =
  | "hidden"
  | "loading"
  | "available"
  | "unavailable"
  | "failed"
  | "manualOnly"
  | "incompatible";
export interface LoaderCapability {
  install: boolean;
  saveInstaller: boolean;
}

/** rawMetadata 白名单值；只允许轻量 JSON 标量。 */
export type LoaderRawMetadataValue = string | number | boolean | null;

/** 前端可见的 artifact 子集；URL、候选下载地址与凭据永不进入该结构。 */
export interface LoaderArtifactMetadata {
  rawMetadata: Record<string, LoaderRawMetadataValue>;
}

export interface LoaderProvider {
  state: LoaderProviderState;
  capability: LoaderCapability;
  error: string;
}

export interface LoaderOption {
  version: string;
  label: string;
  releaseTime: string;
  modifiedTime: string;
  stable: boolean | null;
  latest: boolean;
  recommended: boolean;
  downloadUrl: string;
  changelogUrl: string;
  capability: LoaderCapability;
  /** 兼容旧后端；仅保留白名单 rawMetadata，供共装兼容筛选读取。 */
  artifact?: LoaderArtifactMetadata;
}

export interface LoaderCatalog {
  forge: LoaderOption[];
  neoforge: LoaderOption[];
  fabric: LoaderOption[];
  liteloader: LoaderOption[];
  optifine: LoaderOption[];
  /** 各加载器的备注（例如「该版本暂不支持」），键为加载器标识。 */
  notes: Record<string, string>;
  providers: Record<LoaderId, LoaderProvider>;
}

/** 单 provider 渐进结果；order 由调用方按 LOADER_IDS 合并。 */
export interface LoaderProviderSlice {
  loader: LoaderId;
  builds: LoaderOption[];
  provider: LoaderProvider;
  /** 后端 10 分钟 provider 缓存命中；本地隐藏行不计入。 */
  cached: boolean;
  elapsedMs: number;
}

/** catalogCache 每到达一个 provider 就发布一次，done 时所有行均已落定。 */
export interface LoaderCatalogProgress {
  catalog: LoaderCatalog;
  settled: number;
  total: number;
  done: boolean;
  changed: LoaderId | null;
}

/** 单个文件的下载进度。 */
export interface DownloadFile {
  name: string;
  /** 0–100。 */
  percent: number;
  done: boolean;
}

/** 下载任务状态机。 */
export type DownloadStatus =
  | "idle"
  | "running"
  | "done"
  | "error"
  | "cancelled";

export type DownloadTaskKind =
  | "version"
  | "modpack"
  | "resource"
  | "installer"
  | "export";

export interface QueuedDownload {
  queueId: string;
  task: Omit<DownloadTask, "taskId">;
  enqueuedAt: number;
  starting: boolean;
}

/** 一次安装任务的标识信息。 */
export interface DownloadTask {
  /** 后端确认的非空 taskId；等待项使用独立 queueId。 */
  taskId: string;
  versionId: string;
  instanceName?: string;
  dir: string;
  loader: string;
  loaderVersion: string | null;
  source: DownloadSource;
  kind?: DownloadTaskKind;
}

/** 安装器另存的身份参数；目标路径只由后端原生对话框选择。 */
export interface InstallerDownloadArgs {
  mcVersion: string;
  loader: string;
  loaderVersion: string;
  source: DownloadSource;
}

export interface DownloadState {
  status: DownloadStatus;
  task: DownloadTask | null;
  /** 当前阶段标题（如「下载资源文件」）。 */
  stage: string;
  /** 阶段细节（如具体文件名 / 说明）。 */
  detail: string;
  /** 当前阶段完成度 0–100；阶段总量未知时为 null，切阶段重新计算。 */
  percent: number | null;
  filesDone: number;
  filesTotal: number;
  /** 字节/秒；未知时为 null。 */
  speedBps: number | null;
  /** 预计剩余毫秒；未知时为 null。 */
  remainingMs: number | null;
  /** 任务开始时间戳（毫秒）；未开始为 null。 */
  startedAt: number | null;
  /** 任务结束时间戳（毫秒）；未完成为 null。 */
  endedAt: number | null;
  /** 逐文件进度（按首次出现顺序）。 */
  files: DownloadFile[];
  /** done / error / cancelled 的说明文案（running 时为取消失败等临时原因）。 */
  message: string;
  /** 完成后气泡是否已淡出（下载中恒为 false）。 */
  bubbleDismissed: boolean;
  /** 后端确认的真实目标，不能用显示名拼接路径。 */
  target: string;
  cancelRequested: boolean;
  /** 当前任务实际接收的网络字节，含重试流量，不含缓存文件。 */
  transferredBytes: number | null;
  completedBytes: number | null;
  totalBytes: number | null;
  stages: DownloadStage[];
}

export interface DownloadStage {
  name: string;
  detail: string;
}

/** 后端活动任务与本会话的真实 FIFO 等待项；启动失败不伪造 taskId。 */
export interface DownloadCenterState {
  tasks: DownloadState[];
  queue: QueuedDownload[];
  selectedTaskId: string | null;
  entrance: number;
  bubbleDismissed: boolean;
  error: string;
  startError: string;
}

/**
 * 左栏全局统计：总进度、速度与剩余文件。
 * 只聚合「活动任务」的真实快照字段，不推算整场完成度、不造分母：
 * - percent = 各活动任务当前阶段进度的算术平均（0–100）；无活动任务为 100。
 * - speedBps = 各活动任务已知速度之和（字节/秒）；无活动任务为 0。
 * - remainingFiles = 各活动任务「阶段总量 - 已完成」之和；无活动任务为 0。
 */
export interface DownloadOverview {
  percent: number;
  speedBps: number;
  remainingFiles: number;
}

export interface TaskSnapshot {
  taskId: string;
  kind?: DownloadTaskKind;
  revision: number;
  target: string;
  label: string;
  status: Exclude<DownloadStatus, "idle">;
  cancelRequested: boolean;
  stage: string;
  detail: string;
  percent: number | null;
  filesDone: number;
  filesTotal: number;
  transferredBytes: number | null;
  completedBytes: number | null;
  totalBytes: number | null;
  speedBps: number | null;
  remainingMs: number | null;
  startedAt: number;
  endedAt: number | null;
  files: DownloadFile[];
  stages: DownloadStage[];
  message: string;
}

/** 归一化后的 `install-event`（只保留契约中给出的字段）。 */
export type InstallEvent =
  | { kind: "stage"; taskId: string; stage: string; detail: string }
  | {
      kind: "progress";
      taskId: string;
      percent: number | null;
      filesDone: number;
      filesTotal: number;
      speedBps: number | null;
      remainingMs: number | null;
    }
  | {
      kind: "file";
      taskId: string;
      name: string;
      percent: number;
      done: boolean;
    }
  | {
      kind: "done";
      taskId: string;
      ok: boolean;
      message: string;
      cancelled: boolean;
      result?: InstalledInstance;
    }
  | { kind: "error"; taskId: string; message: string };

export interface InstalledInstance {
  root: string;
  versionId: string;
  displayName: string;
  source: unknown;
}

/** 「资源」tab 的独立页（侧边栏切换）。 */
export type DownloadPage =
  | "vanilla"
  | "mod"
  | "modpack"
  | "datapack"
  | "resourcepack"
  | "shaderpack"
  | "installed";

/** 社区资源分类：具备在线检索来源的五类。 */
export type CommunityCategory = Exclude<DownloadPage, "vanilla" | "installed">;

/** 当前实例的检索过滤条件（来自 `instance_scan` 的真实数据）。 */
export interface InstanceFilter {
  /** `.minecraft` 根目录绝对路径。 */
  dir: string;
  /** 目录名（用于展示）。 */
  label: string;
  /** 从实例 id 中提取的 MC 版本号（如 `1.20.1`）。 */
  mcVersion: string;
  /** 加载器标识；`vanilla` 表示无加载器。 */
  loader: string;
}
