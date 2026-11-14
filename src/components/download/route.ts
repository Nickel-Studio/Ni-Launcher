/**
 * 下载详情页的导航入口。
 *
 * `downloads` 是 `src/lib/nav.ts` 中的合法非 tab 路由，由 `App.tsx` 渲染
 * `<DownloadDetail/>`。本模块只保留一个用于兼容旧调用方的本地开关，
 * 打开 / 关闭都同步到统一路由；返回目标由 nav 记录，保证从任意界面进入后
 * 都能回到下载中心之前的界面。
 */
import { useSyncExternalStore } from "react";
import { returnFromDownloads, setRoute } from "../../lib/nav";

const listeners = new Set<() => void>();
let open = false;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): boolean {
  return open;
}

function emit(): void {
  for (const listener of listeners) listener();
}

function setOpen(next: boolean): void {
  if (next === open) return;
  open = next;
  emit();
}

/** 打开下载详情页（气泡点击 / 页内按钮）。 */
export function openDownloadDetail(): void {
  setOpen(true);
  setRoute("downloads");
}

/** 关闭下载详情页，返回进入下载中心前的界面。 */
export function closeDownloadDetail(): void {
  setOpen(false);
  returnFromDownloads();
}

/** 是否是「下载详情」视图（配合 App.tsx 未接入路由时使用）。 */
export function useDownloadDetailOpen(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
