import { useSyncExternalStore } from "react";
import { setVersionFocus, type VersionFocus } from "./instances";

/**
 * 应用路由（唯一真源）。
 *
 * 分两类：
 *   - **tab 路由**（`TAB_ROUTES`）：顶栏常驻的四个页面 —— 首页 / 资源 / 设置 / 更多。
 *     顶栏 tab 与它们一一对应，切换时顶栏的滑动胶囊滑到该项。
 *   - **非 tab 路由**：不属于任何顶栏 tab 的**独立页**（实例 / 版本设置 / 下载中心…），
 *     从首页等页面内的按钮进入，页面内自带返回/上游入口；此时顶栏不平铺任何高亮
 *     （`TAB_ROUTES.indexOf(route) === -1`，滑动胶囊保持隐藏）。
 *
 * 为什么两类合用一个 store：路由是全局单例状态，顶栏、页面与启动流程都要读写同一个值；
 * 拆成两个 store 反而会出现「谁覆盖谁」的竞态。
 */
export type Route =
  | "home"
  | "resources"
  | "settings"
  | "more"
  | "instances"
  | "versionSettings"
  | "downloads";

/** 顶栏 tab 路由；顺序 = 顶栏展示顺序。红线圈定这四项，不得增删。 */
export const TAB_ROUTES: readonly Route[] = [
  "home",
  "resources",
  "settings",
  "more",
];

/** 全部合法路由（`setRoute` 的运行时白名单）。 */
const ALL_ROUTES: readonly Route[] = [
  ...TAB_ROUTES,
  // 非 tab 路由：无顶栏归属的独立页（从首页按钮进入）。
  "instances",
  "versionSettings",
  "downloads",
];

const listeners = new Set<() => void>();
let currentRoute: Route = "home";
/**
 * 进入下载中心前的界面。下载任务清空后要「强制返回」到这里，而不是固定回到资源页：
 * 用户可能从首页气泡、版本设置或导出面板进入。只记录非下载中心的路由，
 * 从下载中心直接跳到其它页面时保留原入口，避免返回目标被覆盖成下载中心自身。
 */
let previousRoute: Route = "home";

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): Route {
  return currentRoute;
}

function emit(): void {
  for (const listener of listeners) listener();
}

/**
 * Imperative route setter (usable outside React too).
 *
 * 只接受 `ALL_ROUTES` 中的值：未知值（例如旧版本残留的 "versions"）一律忽略，
 * 避免把界面切到一个没有页面接线的路由、导致内容区空白。
 */
export function setRoute(route: Route): void {
  if (!ALL_ROUTES.includes(route) || route === currentRoute) return;
  if (currentRoute !== "downloads") previousRoute = currentRoute;
  currentRoute = route;
  emit();
}

/** 从下载中心返回进入前的界面；入口本身是下载中心时回退到资源页。 */
export function returnFromDownloads(): void {
  setRoute(previousRoute === "downloads" ? "resources" : previousRoute);
}

/** 目标身份和导航一起更新；展示名永不参与目录寻址。 */
export function openVersionSettings(target: VersionFocus): void {
  if (!target.root.trim() || !target.versionId.trim()) return;
  setVersionFocus(target);
  setRoute("versionSettings");
}

export const RESOURCE_MODS_EVENT = "nowhere:open-resource-mods";
let resourceModsTarget: VersionFocus | null = null;

/** 浏览当前实例的 Mod，不改动首页启动实例；接收方在 effect 中消费。 */
export function openResourceMods(target: VersionFocus): void {
  if (!target.root.trim() || !target.versionId.trim()) return;
  resourceModsTarget = { root: target.root, versionId: target.versionId };
  setRoute("resources");
  window.dispatchEvent(new Event(RESOURCE_MODS_EVENT));
}

export function consumeResourceModsTarget(): VersionFocus | null {
  const target = resourceModsTarget;
  resourceModsTarget = null;
  return target;
}

/** Current route value (non-reactive). */
export function getRoute(): Route {
  return currentRoute;
}

/**
 * Shared application navigation store.
 * Returns the active route and a setter. Any component calling this hook
 * (title bar tabs, page bodies...) stays in sync automatically.
 */
export function useAppNav(): { route: Route; setRoute: (r: Route) => void } {
  const route = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  return { route, setRoute };
}
