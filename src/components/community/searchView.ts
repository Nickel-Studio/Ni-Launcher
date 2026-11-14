/**
 * 社区资源 · 检索视图与共享副作用。
 *
 * `CommunitySearchView` 是引擎页与合并结果区的统一渲染契约：
 * CommunitySearchPage 负责把「单源 hook」或「合并视图」收敛成它，
 * 引擎页只消费、不关心数据来自哪个分支。
 * `useDetailInstance` 是检索页与详情页共用的静默实例识别（目录 + 版本·加载器）。
 */
import { useEffect, useState } from "react";
import { loadResourceInstance } from "../resources/ipc";
import { useLaunchInstance, type VersionFocus } from "../../lib/instances";
import { useDownloadState } from "../download/store";
import type { ModrinthHit } from "../modrinth/types";
import type { DetailInstance } from "./ProjectDetailPage";

/** 列表页状态机：`blocked` = 当前引擎在该实例下不可检索。 */
export type CommunitySearchState =
  | "loading"
  | "ready"
  | "empty"
  | "error"
  | "blocked";

/** 引擎页与合并结果区统一的检索视图（由 CommunitySearchPage 收敛提供）。 */
export interface CommunitySearchView {
  state: CommunitySearchState;
  /** 失败 / 空态的具体原因（可执行的中文提示）。 */
  message: string;
  hits: ModrinthHit[];
  /** 已知命中总数；合并模式下拿不到可靠 total 时为 null（计数行不显示数字）。 */
  total: number | null;
  /** 是否还有下一页（合并模式 = 任一来源仍有下一页）。 */
  hasMore: boolean;
  /** 按当前条件重新检索（错误态重试）。 */
  reload: () => void;
}

const EMPTY_INSTANCE: DetailInstance = {
  root: "",
  versionId: "",
  dir: "",
  label: "",
  mcVersion: "",
  loader: "",
};

/** 切页、安装结束与目标实例变化后重读；显式导航目标优先于首页启动实例。 */
export function useDetailInstance(
  refreshKey: unknown = null,
  target?: VersionFocus | null
): DetailInstance {
  const { instance: launch } = useLaunchInstance();
  const currentTarget = target ?? launch;
  const { endedAt } = useDownloadState();
  const [snapshot, setSnapshot] = useState<{
    key: unknown;
    target: VersionFocus | null;
    endedAt: typeof endedAt;
    value: DetailInstance;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void loadResourceInstance(currentTarget ?? undefined)
      .then((current) => {
        if (cancelled) return;
        setSnapshot({
          key: refreshKey,
          target: currentTarget,
          endedAt,
          value: current ?? EMPTY_INSTANCE,
        });
      })
      .catch(() => {
        if (!cancelled)
          setSnapshot({
            key: refreshKey,
            target: currentTarget,
            endedAt,
            value: EMPTY_INSTANCE,
          });
      });
    return () => {
      cancelled = true;
    };
  }, [refreshKey, currentTarget, endedAt]);

  return snapshot &&
    snapshot.key === refreshKey &&
    snapshot.target === currentTarget &&
    snapshot.endedAt === endedAt
    ? snapshot.value
    : EMPTY_INSTANCE;
}
