import { useSyncExternalStore } from "react";

/**
 * 启动实例 —— 首页启动按钮真正要启动的那个实例。
 *
 * 实现方式与 `src/lib/nav.ts` 一致（模块级 store + useSyncExternalStore）：
 * 任意组件调用 `useLaunchInstance()` 都能自动同步同一个值。
 * 持久化到 localStorage 的 `nowhere.instance.launch`；内容缺失 / JSON 损坏 /
 * 存储不可用一律视为「未选择」（`null`），绝不抛错 —— 它只是一个启动偏好，
 * 任何情况下都不该让界面崩掉或白屏。
 */
export interface LaunchInstance {
  /** 该实例所属的 `.minecraft` 根目录（绝对路径）。 */
  root: string;
  /** 版本 id，对应 `<root>/versions/<id>`。 */
  versionId: string;
  /** 展示元数据；不参与目录身份或启动参数。 */
  displayName?: string;
  /** 加载器标识（vanilla / forge / neoforge / fabric / optifine / liteloader …）。 */
  loader: string;
  /** Blocks 图标键（grass / anvil / fabric …）。 */
  icon: string;
}

const STORAGE_KEY = "nowhere.instance.launch";

const listeners = new Set<() => void>();

function isLaunchInstance(value: unknown): value is LaunchInstance {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.root === "string" &&
    typeof record.versionId === "string" &&
    typeof record.loader === "string" &&
    typeof record.icon === "string"
  );
}

function readStored(): LaunchInstance | null {
  try {
    if (typeof window === "undefined") return null;
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!isLaunchInstance(parsed)) return null;
    return {
      root: parsed.root.trim(),
      versionId: parsed.versionId.trim(),
      displayName:
        typeof parsed.displayName === "string"
          ? parsed.displayName.trim()
          : parsed.versionId.trim(),
      loader: parsed.loader.trim(),
      icon: parsed.icon.trim(),
    };
  } catch {
    // JSON 损坏 / localStorage 被禁用：当作「未选择」处理。
    return null;
  }
}

let current: LaunchInstance | null = readStored();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): LaunchInstance | null {
  return current;
}

function emit(): void {
  for (const listener of listeners) listener();
}

/** 设置（传 `null` 即清空）启动实例；写盘失败时仍保留内存态。 */
export function setLaunchInstance(instance: LaunchInstance | null): void {
  const next: LaunchInstance | null = instance
    ? {
        root: instance.root.trim(),
        versionId: instance.versionId.trim(),
        displayName: instance.displayName?.trim() || instance.versionId.trim(),
        loader: instance.loader.trim(),
        icon: instance.icon.trim(),
      }
    : null;

  current = next;
  try {
    if (next) window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    else window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* 存储不可用：只保留内存态 */
  }
  emit();
}

/** 当前启动实例（非响应式读取）。 */
export function getLaunchInstance(): LaunchInstance | null {
  return current;
}

/**
 * 启动实例 store：返回当前值与其 setter，跨组件自动同步。
 * 供首页启动流程消费（未选择时 `instance` 为 `null`）。
 */
export function useLaunchInstance(): {
  instance: LaunchInstance | null;
  setLaunchInstance: (i: LaunchInstance | null) => void;
} {
  const instance = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  return { instance, setLaunchInstance };
}

/**
 * 版本焦点 —— 「版本设置」页当前要查看 / 编辑的那个版本实例。
 *
 * 从实例页点「版本设置」时写入（`setVersionFocus({ root, versionId })`），
 * 进入 `versionSettings` 独立页后由该页读取消费。实现方式与 `useLaunchInstance`
 * 完全一致（模块级 store + useSyncExternalStore + localStorage 持久化），
 * 键为 `nowhere.version.focus`；内容缺失 / JSON 损坏 / 存储不可用一律视为
 * 「无焦点」（`null`），它只是一个查看偏好，任何情况下都不抛错、不白屏。
 */
export interface VersionFocus {
  /** 该实例所属的 `.minecraft` 根目录（绝对路径）。 */
  root: string;
  /** 版本 id，对应 `<root>/versions/<id>`。 */
  versionId: string;
}

const FOCUS_STORAGE_KEY = "nowhere.version.focus";

const focusListeners = new Set<() => void>();

function isVersionFocus(value: unknown): value is VersionFocus {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.root === "string" &&
    record.root.trim() !== "" &&
    typeof record.versionId === "string" &&
    record.versionId.trim() !== ""
  );
}

function readStoredFocus(): VersionFocus | null {
  try {
    if (typeof window === "undefined") return null;
    const raw = window.localStorage.getItem(FOCUS_STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!isVersionFocus(parsed)) return null;
    return {
      root: parsed.root.trim(),
      versionId: parsed.versionId.trim(),
    };
  } catch {
    // JSON 损坏 / localStorage 被禁用：当作「无焦点」处理。
    return null;
  }
}

let currentFocus: VersionFocus | null = readStoredFocus();

function subscribeFocus(listener: () => void): () => void {
  focusListeners.add(listener);
  return () => {
    focusListeners.delete(listener);
  };
}

function getFocusSnapshot(): VersionFocus | null {
  return currentFocus;
}

function emitFocus(): void {
  for (const listener of focusListeners) listener();
}

/** 设置（传 `null` 即清空）版本焦点；写盘失败时仍保留内存态。 */
export function setVersionFocus(focus: VersionFocus | null): void {
  const next: VersionFocus | null = focus
    ? {
        root: focus.root.trim(),
        versionId: focus.versionId.trim(),
      }
    : null;

  currentFocus = next;
  try {
    if (next) {
      window.localStorage.setItem(FOCUS_STORAGE_KEY, JSON.stringify(next));
    } else {
      window.localStorage.removeItem(FOCUS_STORAGE_KEY);
    }
  } catch {
    /* 存储不可用：只保留内存态 */
  }
  emitFocus();
}

/** 当前版本焦点（非响应式读取）。 */
export function getVersionFocus(): VersionFocus | null {
  return currentFocus;
}

/**
 * 版本焦点 store：返回当前值与其 setter，跨组件自动同步。
 * 供版本设置独立页消费（无焦点时 `focus` 为 `null`，页面自行引导回实例页）。
 */
export function useVersionFocus(): {
  focus: VersionFocus | null;
  setVersionFocus: (f: VersionFocus | null) => void;
} {
  const focus = useSyncExternalStore(
    subscribeFocus,
    getFocusSnapshot,
    getFocusSnapshot
  );
  return { focus, setVersionFocus };
}
