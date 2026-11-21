/**
 * 下载中心 · 与 Rust 后端的全部 IPC 通道。
 *
 * 集中在一个模块的理由：这些命令由后端并行实现，缺失或失败时必须整体可降级，
 * 调用点只负责「调用 + 归一化」，失败时抛出可展示的中文错误（`describeInstallError`），
 * 由页面决定如何呈现，绝不白屏或崩溃。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "../../lib/settings";
import { parseTaskSnapshots } from "./snapshots";
import { loadResourceInstance, resolveInstanceRoot } from "../resources/ipc";
import {
  normalizeLoaders,
  normalizeManifest,
  parseInstallEvent,
} from "./parse";
import { LOADER_IDS } from "./types";
import type {
  DownloadSource,
  InstallEvent,
  InstanceFilter,
  LoaderCatalog,
  LoaderId,
  LoaderProviderSlice,
  RemoteManifest,
} from "./types";

/** `install_version` 的入参。 */
export interface InstallArgs {
  dir: string;
  versionId: string;
  instanceName?: string;
  /** 加载器标识；空串表示原版（不安装加载器）。 */
  loader: string;
  /** 加载器版本；原版为 null。 */
  loaderVersion: string | null;
  source: DownloadSource;
}

/** 拉取远端版本清单（`list_remote_versions`）。 */
export async function listRemoteVersions(
  source: DownloadSource
): Promise<RemoteManifest> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("list_remote_versions", { source });
  return normalizeManifest(raw);
}

/** `list_loaders` / 单 provider 命令共用的请求选项。 */
export interface LoaderCatalogOptions {
  releaseTime?: string;
  forceRefresh?: boolean;
  operationId?: string;
}

/** 拉取某 MC 版本可用的加载器清单（`list_loaders`，兼容全量调用）。 */
export async function listLoaders(
  mcVersion: string,
  source: DownloadSource,
  options?: LoaderCatalogOptions
): Promise<LoaderCatalog> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("list_loaders", {
    mcVersion,
    source,
    ...options,
  });
  return normalizeLoaders(raw);
}

function parseLoaderProviderSlice(raw: unknown): LoaderProviderSlice {
  let data = raw;
  if (typeof raw === "string") {
    try {
      data = JSON.parse(raw) as unknown;
    } catch {
      throw new Error("加载器清单格式无法识别，请重试。");
    }
  }
  if (!data || typeof data !== "object" || Array.isArray(data))
    throw new Error("加载器清单格式无法识别，请重试。");
  const record = data as Record<string, unknown>;
  const loader = LOADER_IDS.find((id) => id === record.loader);
  if (!loader) throw new Error("加载器清单缺少提供方标识，请重试。");
  const catalog = normalizeLoaders({
    [loader]: Array.isArray(record.builds) ? record.builds : [],
    providers: { [loader]: record.provider },
  });
  return {
    loader,
    builds: catalog[loader],
    provider: catalog.providers[loader],
    cached: record.cached === true,
    elapsedMs:
      typeof record.elapsedMs === "number" && Number.isFinite(record.elapsedMs)
        ? Math.max(0, record.elapsedMs)
        : 0,
  };
}

/** 单 provider 渐进请求；慢 provider 不阻塞其它行，forceRefresh 仅强刷本行。 */
export async function listLoaderProvider(
  mcVersion: string,
  loader: LoaderId,
  source: DownloadSource,
  options?: LoaderCatalogOptions
): Promise<LoaderProviderSlice> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("list_loader_provider", {
    mcVersion,
    loader,
    source,
    ...options,
  });
  return parseLoaderProviderSlice(raw);
}

/** 页面进入时预热；命令立即返回，实际清单在后台共享 provider 缓存。 */
export async function preloadLoaderCatalog(
  mcVersion: string,
  source: DownloadSource,
  options?: Omit<LoaderCatalogOptions, "operationId">
): Promise<void> {
  if (!isTauri()) return;
  await invoke("preload_loader_catalog", {
    mcVersion,
    source,
    ...options,
  });
}

/** UI 专用入口：返回 taskId，取消保存对话框返回 null。 */
export async function saveLoaderInstallerManaged(
  args: import("./types").InstallerDownloadArgs
): Promise<string | null> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  return invoke<string | null>("save_loader_installer_managed", { ...args });
}

/** 返回的是任务身份，不是安装完成结果；终态由安装事件通知。 */
export async function installVersion(args: InstallArgs): Promise<string> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  const raw = await invoke<string>("install_version", {
    dir: args.dir,
    versionId: args.versionId,
    instanceName: args.instanceName ?? null,
    loader: args.loader,
    loaderVersion: args.loaderVersion,
    source: args.source,
  });
  return typeof raw === "string" ? raw.trim() : "";
}

export async function cancelInstall(taskId: string): Promise<void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  await invoke("install_cancel", { taskId });
}

/** 可重建的真实活动任务与最近终态，进度由后端统一采样。 */
export async function listTaskSnapshots(): Promise<
  import("./types").TaskSnapshot[]
> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  return parseTaskSnapshots(await invoke<unknown>("app_tasks_snapshot"));
}

/**
 * 解析本次安装的目标目录。
 *
 * 首选 `instances_get` 的 active 实例（与版本页「当前实例」语义一致）；
 * 注册表为空时调用 `instance_ensure_default` 初始化默认实例目录。
 * 两者都拿不到则返回 null，由调用方给出可执行提示。
 */
export async function ensureInstanceDir(): Promise<string | null> {
  const active = await resolveInstanceRoot();
  if (active) return active.dir;

  if (!isTauri()) return null;
  try {
    const raw = await invoke<string>("instance_ensure_default");
    const dir = typeof raw === "string" ? raw.trim() : "";
    return dir || null;
  } catch {
    return null;
  }
}

/** 社区筛选只读取已选启动实例，与普通资源安装目标保持一致。 */
export async function loadInstanceFilter(): Promise<InstanceFilter | null> {
  return loadResourceInstance();
}

/**
 * 订阅安装事件流（`install-event`）。
 * 非 Tauri 环境（纯浏览器预览）或事件插件不可用时返回空取消函数，不产生未处理的 Promise 拒绝。
 */
export function subscribeInstallEvents(
  onEvent: (event: InstallEvent) => void
): () => void {
  if (!isTauri()) return () => {};
  let disposed = false;
  const pending = listen<unknown>("install-event", (event) => {
    if (disposed) return;
    const parsed = parseInstallEvent(event?.payload);
    if (parsed) onEvent(parsed);
  });
  void pending.catch(() => {});

  return () => {
    disposed = true;
    void pending.then((unlisten) => unlisten()).catch(() => {});
  };
}

/** 启动任务前等待订阅真正建立，避免极快失败事件丢失。 */
export async function subscribeInstallEventsReady(
  onEvent: (event: InstallEvent) => void
): Promise<() => void> {
  if (!isTauri()) throw new Error("未连接到启动器后端");
  return listen<unknown>("install-event", (event) => {
    const parsed = parseInstallEvent(event.payload);
    if (parsed) onEvent(parsed);
  });
}
