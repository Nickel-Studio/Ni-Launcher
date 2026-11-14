import { listLoaderProvider } from "../download/ipc";
import {
  LOADER_IDS,
  type DownloadSource,
  type LoaderCatalog,
  type LoaderCatalogProgress,
  type LoaderId,
  type LoaderProvider,
  type LoaderProviderSlice,
  type RemoteManifest,
  type RemoteVersion,
} from "../download/types";

const CACHE_TTL_MS = 10 * 60 * 1000;
const MAX_CACHED_CATALOGS = 40;
const MAX_CACHED_PROVIDER_CATALOGS = MAX_CACHED_CATALOGS * LOADER_IDS.length;
const PRELOAD_DEBOUNCE_MS = 160;
const MAX_QUEUED_PRELOADS = 4;
const PRELOAD_OPERATION_ID = "preload";

const catalogs = new Map<string, { savedAt: number; catalog: LoaderCatalog }>();
const providerCatalogs = new Map<
  string,
  { savedAt: number; slice: LoaderProviderSlice }
>();

type CatalogRun = {
  key: string;
  id: number;
  forceRefresh: boolean;
  request: Promise<LoaderCatalog>;
  resolve: (catalog: LoaderCatalog) => void;
  reject: (reason: unknown) => void;
  listeners: Set<(progress: LoaderCatalogProgress) => void>;
  catalog: LoaderCatalog;
  settled: Set<LoaderId>;
  firstError: string;
  finished: boolean;
};

const pending = new Map<string, CatalogRun>();
const latestRequest = new Map<string, number>();
let requestId = 0;

type PreloadRequest = {
  key: string;
  version: RemoteVersion;
  source: DownloadSource;
};

const preloadQueue: PreloadRequest[] = [];
const queuedPreloads = new Set<string>();
let preloadActive = false;
let scheduledPreload: PreloadRequest | null = null;
let preloadTimer: ReturnType<typeof setTimeout> | null = null;

function catalogKey(version: RemoteVersion, source: DownloadSource): string {
  return JSON.stringify([version.id, version.releaseTime, source]);
}

function providerKey(
  version: RemoteVersion,
  source: DownloadSource,
  loader: LoaderId
): string {
  return `${catalogKey(version, source)}\u0000${loader}`;
}

function catalogComplete(catalog: LoaderCatalog): boolean {
  return LOADER_IDS.every(
    (loader) => !["failed", "loading"].includes(catalog.providers[loader].state)
  );
}

function allVisibleProvidersFailed(catalog: LoaderCatalog): boolean {
  let failed = false;
  for (const loader of LOADER_IDS) {
    const state = catalog.providers[loader].state;
    if (state === "hidden") continue;
    if (state !== "failed") return false;
    failed = true;
  }
  return failed;
}

function saveCatalog(key: string, catalog: LoaderCatalog): void {
  catalogs.delete(key);
  catalogs.set(key, { savedAt: Date.now(), catalog });
  if (catalogs.size > MAX_CACHED_CATALOGS) {
    const oldest = catalogs.keys().next().value;
    if (oldest !== undefined) catalogs.delete(oldest);
  }
}

function saveProviderCatalog(key: string, slice: LoaderProviderSlice): void {
  providerCatalogs.delete(key);
  providerCatalogs.set(key, { savedAt: Date.now(), slice });
  if (providerCatalogs.size > MAX_CACHED_PROVIDER_CATALOGS) {
    const oldest = providerCatalogs.keys().next().value;
    if (oldest !== undefined) providerCatalogs.delete(oldest);
  }
}

function cachedProviderCatalog(
  version: RemoteVersion,
  source: DownloadSource,
  loader: LoaderId
): LoaderProviderSlice | null {
  const entry = providerCatalogs.get(providerKey(version, source, loader));
  return entry && Date.now() - entry.savedAt < CACHE_TTL_MS
    ? entry.slice
    : null;
}

export function cachedCatalog(
  version: RemoteVersion,
  source: DownloadSource
): LoaderCatalog | null {
  const entry = catalogs.get(catalogKey(version, source));
  return entry && Date.now() - entry.savedAt < CACHE_TTL_MS
    ? entry.catalog
    : null;
}

export function initialCatalog(version: RemoteVersion): LoaderCatalog {
  const traditional = /^1\.(\d+)(?:\.(\d+))?$/.exec(version.id);
  const minor = traditional ? Number(traditional[1]) : null;
  const patch = traditional ? Number(traditional[2] ?? 0) : 0;
  const released = Date.parse(version.releaseTime);
  const oldNeo = Number.isFinite(released)
    ? released < Date.parse("2023-06-11T00:00:00Z")
    : minor !== null && (minor < 20 || (minor === 20 && patch < 1));
  const provider = (hidden: boolean, hasEngine = true): LoaderProvider => ({
    state: hidden ? "hidden" : hasEngine ? "loading" : "unavailable",
    capability: { install: false, saveInstaller: false },
    error: "",
  });
  return {
    forge: [],
    neoforge: [],
    fabric: [],
    liteloader: [],
    optifine: [],
    notes: {},
    providers: {
      forge: provider(/^(a1\.|b1\.|rd-|c0\.|inf-)/.test(version.id)),
      neoforge: provider(oldNeo),
      fabric: provider(minor !== null && minor <= 13),
      // 1.13 起不再请求 LiteLoader；提供方清单也不提供 1.5/1.6。
      liteloader: provider(minor === null || minor >= 13 || minor <= 6),
      optifine: provider(false),
    },
  };
}

function applySlice(
  catalog: LoaderCatalog,
  slice: LoaderProviderSlice
): LoaderCatalog {
  const next: LoaderCatalog = {
    ...catalog,
    providers: {
      ...catalog.providers,
      [slice.loader]: slice.provider,
    },
  };
  next[slice.loader] = slice.builds;
  return next;
}

function markLoading(catalog: LoaderCatalog, loader: LoaderId): LoaderCatalog {
  return {
    ...catalog,
    providers: {
      ...catalog.providers,
      [loader]: {
        ...catalog.providers[loader],
        state: "loading",
        error: "",
      },
    },
  };
}

function markFailed(
  catalog: LoaderCatalog,
  loader: LoaderId,
  message: string
): LoaderCatalog {
  return {
    ...catalog,
    providers: {
      ...catalog.providers,
      [loader]: {
        state: "failed",
        capability: { install: false, saveInstaller: false },
        error: message,
      },
    },
  };
}

function errorText(reason: unknown): string {
  const message =
    reason instanceof Error
      ? reason.message
      : typeof reason === "string"
      ? reason
      : "";
  return message.trim() || "加载器清单加载失败，请重试。";
}

function progressOf(
  run: CatalogRun,
  changed: LoaderId | null
): LoaderCatalogProgress {
  return {
    catalog: run.catalog,
    settled: run.settled.size,
    total: LOADER_IDS.length,
    done: run.finished,
    changed,
  };
}

function emit(run: CatalogRun, changed: LoaderId | null): void {
  const progress = progressOf(run, changed);
  for (const listener of run.listeners) {
    try {
      listener(progress);
    } catch {
      // 单个订阅者的渲染异常不能阻断其它 provider 的到达。
    }
  }
}

function isCurrent(run: CatalogRun): boolean {
  return latestRequest.get(run.key) === run.id;
}

function cleanup(run: CatalogRun): void {
  if (pending.get(run.key)?.id === run.id) {
    pending.delete(run.key);
    latestRequest.delete(run.key);
  }
}

function finish(run: CatalogRun, changed: LoaderId | null): void {
  if (run.finished) return;
  run.finished = true;
  if (!isCurrent(run)) {
    run.resolve(run.catalog);
    return;
  }

  const allFailed = allVisibleProvidersFailed(run.catalog);
  if (!allFailed && catalogComplete(run.catalog)) {
    saveCatalog(run.key, run.catalog);
  }
  emit(run, changed);
  cleanup(run);
  if (allFailed) {
    run.reject(new Error(run.firstError || "加载器清单加载失败，请重试。"));
  } else {
    run.resolve(run.catalog);
  }
}

function settle(run: CatalogRun, loader: LoaderId): void {
  if (run.settled.has(loader)) return;
  run.settled.add(loader);
  if (run.settled.size === LOADER_IDS.length) {
    finish(run, loader);
  } else if (isCurrent(run)) {
    emit(run, loader);
  }
}

function requestProvider(
  run: CatalogRun,
  version: RemoteVersion,
  source: DownloadSource,
  loader: LoaderId,
  operationId: string
): void {
  void listLoaderProvider(version.id, loader, source, {
    releaseTime: version.releaseTime,
    forceRefresh: run.forceRefresh,
    operationId: `${operationId}:${loader}`,
  })
    .then((slice) => {
      if (run.finished) return;
      run.catalog = applySlice(run.catalog, slice);
      if (
        isCurrent(run) &&
        !["failed", "loading"].includes(slice.provider.state)
      ) {
        saveProviderCatalog(providerKey(version, source, loader), slice);
      }
      settle(run, loader);
    })
    .catch((reason: unknown) => {
      if (run.finished) return;
      const message = errorText(reason);
      if (!run.firstError) run.firstError = message;
      run.catalog = markFailed(run.catalog, loader, message);
      settle(run, loader);
    });
}

function startCatalogRun(
  run: CatalogRun,
  version: RemoteVersion,
  source: DownloadSource,
  operationId: string
): void {
  const local = initialCatalog(version);
  run.catalog = cachedCatalog(version, source) ?? local;
  const requested: LoaderId[] = [];
  let cached = 0;
  for (const loader of LOADER_IDS) {
    // 本地粗筛与后端 query_plan 同口径；不适用提供方不发网络请求。
    if (local.providers[loader].state === "hidden") {
      run.catalog = applySlice(run.catalog, {
        loader,
        builds: [],
        provider: local.providers[loader],
        cached: true,
        elapsedMs: 0,
      });
      run.settled.add(loader);
      continue;
    }
    const slice = run.forceRefresh
      ? null
      : cachedProviderCatalog(version, source, loader);
    if (slice) {
      run.catalog = applySlice(run.catalog, slice);
      run.settled.add(loader);
      cached += 1;
    } else {
      run.catalog = markLoading(run.catalog, loader);
      requested.push(loader);
    }
  }

  if (cached > 0 && requested.length > 0) emit(run, null);
  if (requested.length === 0) {
    finish(run, null);
    return;
  }
  for (const loader of requested) {
    requestProvider(run, version, source, loader, operationId);
  }
}

/**
 * 渐进拉取清单：缓存命中行立即发布，其余 provider 独立到达。
 * 同一输入的普通请求合并；手动刷新创建新 generation，旧响应不得写缓存。
 */
export function requestCatalog(
  version: RemoteVersion,
  source: DownloadSource,
  forceRefresh: boolean,
  operationId: string,
  onProgress?: (progress: LoaderCatalogProgress) => void
): Promise<LoaderCatalog> {
  if (operationId !== PRELOAD_OPERATION_ID) {
    // 用户主动进入选择器时抢占尚未开始的预热，但不取消已在途请求或既有缓存。
    cancelScheduledPreload();
    preloadQueue.length = 0;
    queuedPreloads.clear();
  }
  const key = catalogKey(version, source);
  const existing = pending.get(key);
  // 普通请求可合并；显式重试必须发出新请求并作废旧 generation。
  if (existing && (!forceRefresh || existing.forceRefresh)) {
    if (onProgress) {
      existing.listeners.add(onProgress);
      try {
        onProgress(progressOf(existing, null));
      } catch {
        // 订阅者异常不能影响已合并的请求。
      }
    }
    return existing.request;
  }

  if (!forceRefresh) {
    const cached = cachedCatalog(version, source);
    if (cached) {
      if (onProgress) {
        try {
          onProgress({
            catalog: cached,
            settled: LOADER_IDS.length,
            total: LOADER_IDS.length,
            done: true,
            changed: null,
          });
        } catch {
          // 缓存命中仍应返回清单，订阅者异常不改变结果。
        }
      }
      return Promise.resolve(cached);
    }
  }

  let resolve!: (catalog: LoaderCatalog) => void;
  let reject!: (reason: unknown) => void;
  const request = new Promise<LoaderCatalog>(
    (resolvePromise, rejectPromise) => {
      resolve = resolvePromise;
      reject = rejectPromise;
    }
  );
  const run: CatalogRun = {
    key,
    id: ++requestId,
    forceRefresh,
    request,
    resolve,
    reject,
    listeners: new Set(onProgress ? [onProgress] : []),
    catalog: initialCatalog(version),
    settled: new Set(),
    firstError: "",
    finished: false,
  };
  pending.set(key, run);
  latestRequest.set(key, run.id);
  startCatalogRun(run, version, source, operationId);
  return request;
}

function drainPreloadQueue(): void {
  if (preloadActive) return;
  const next = preloadQueue.shift();
  if (!next) return;
  queuedPreloads.delete(next.key);
  preloadActive = true;
  void requestCatalog(next.version, next.source, false, PRELOAD_OPERATION_ID)
    .catch(() => {})
    .finally(() => {
      preloadActive = false;
      drainPreloadQueue();
    });
}

function enqueuePreload(version: RemoteVersion, source: DownloadSource): void {
  const key = catalogKey(version, source);
  if (
    queuedPreloads.has(key) ||
    pending.has(key) ||
    cachedCatalog(version, source)
  ) {
    return;
  }
  if (preloadQueue.length >= MAX_QUEUED_PRELOADS) return;
  preloadQueue.push({ key, version, source });
  queuedPreloads.add(key);
  drainPreloadQueue();
}

/** 页面预热最多串行处理四个清单；失败静默，进入选择器后仍走正常错误路径。 */
export function preloadCatalog(
  version: RemoteVersion,
  source: DownloadSource
): void {
  enqueuePreload(version, source);
}

/** 指针/焦点预热做短暂停留防抖，快速掠过版本列表不会形成请求风暴。 */
export function schedulePreloadCatalog(
  version: RemoteVersion,
  source: DownloadSource
): void {
  cancelScheduledPreload();
  scheduledPreload = { key: catalogKey(version, source), version, source };
  preloadTimer = setTimeout(() => {
    const next = scheduledPreload;
    scheduledPreload = null;
    preloadTimer = null;
    if (next) enqueuePreload(next.version, next.source);
  }, PRELOAD_DEBOUNCE_MS);
}

export function cancelScheduledPreload(): void {
  if (preloadTimer !== null) {
    clearTimeout(preloadTimer);
    preloadTimer = null;
  }
  scheduledPreload = null;
}

/**
 * 页面进入后只预热最新正式版与预览版，各一次。
 * 已排队、在途或缓存命中的清单由 catalogCache 自动合并。
 */
export function preloadLatestCatalogs(
  manifest: RemoteManifest,
  source: DownloadSource
): void {
  const selected = new Map<string, RemoteVersion>();
  const addById = (id: string): boolean => {
    if (!id || selected.has(id)) return false;
    const version = manifest.versions.find((item) => item.id === id);
    if (!version) return false;
    selected.set(version.id, version);
    return true;
  };
  const addFirst = (type: string): void => {
    const version = manifest.versions.find((item) => item.type === type);
    if (version) selected.set(version.id, version);
  };

  const hasRelease = addById(manifest.latestRelease);
  const hasSnapshot = addById(manifest.latestSnapshot);
  if (!hasRelease) addFirst("release");
  if (!hasSnapshot) addFirst("snapshot");

  for (const version of selected.values()) {
    preloadCatalog(version, source);
  }
}
