import { useCallback, useEffect, useState } from "react";
import { describeScanError } from "./parse";
import { resolveResourceRoot, scanKind } from "./ipc";
import {
  CATEGORIES,
  type InstanceRoot,
  type ResourceCategory,
  type ResourceEntry,
  type ResourceKind,
} from "./types";

/** 页面主区域的状态机。 */
export type ResourcesState = "loading" | "ready" | "empty" | "error";

/** 空状态的真实原因，决定文案（不是兜底话术）。 */
export type EmptyReason = "noRoot" | "noSource" | "noItems";

export interface UseResources {
  state: ResourcesState;
  reason: EmptyReason | null;
  entries: ResourceEntry[];
  /** 部分种类扫描失败的提示（有成功条目时作为提醒条展示）。 */
  issues: string[];
  root: InstanceRoot | null;
  /** 各分类已扫描到的条目数量；未知（未扫描）时该键不存在。 */
  counts: Partial<Record<ResourceKind, number>>;
  reload: () => void;
}

/** 按「分类顺序 → 名称」排序，让「全部」列表自然按种类分组。 */
function compareByKindThenName(
  order: Map<ResourceKind, number>
): (a: ResourceEntry, b: ResourceEntry) => number {
  return (a, b) => {
    const delta = (order.get(a.kind) ?? 0) - (order.get(b.kind) ?? 0);
    return delta !== 0 ? delta : a.name.localeCompare(b.name, "zh-Hans-CN");
  };
}

/**
 * 本地资源扫描状态。只读取本地文件系统（经 Rust 命令），不访问任何外部接口。
 * 任何一步失败都会落到明确的空/错误状态，界面永不白屏。
 */
export function useResources(category: ResourceCategory, target?: { root: string; versionId: string }): UseResources {
  const targetRoot = target?.root;
  const targetVersion = target?.versionId;
  const [state, setState] = useState<ResourcesState>("loading");
  const [reason, setReason] = useState<EmptyReason | null>(null);
  const [entries, setEntries] = useState<ResourceEntry[]>([]);
  const [issues, setIssues] = useState<string[]>([]);
  const [root, setRoot] = useState<InstanceRoot | null>(null);
  const [counts, setCounts] = useState<Partial<Record<ResourceKind, number>>>(
    {}
  );
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    let cancelled = false;
    const kinds = CATEGORIES.find((item) => item.id === category)?.kinds ?? [];

    setState("loading");
    setReason(null);
    setEntries([]);
    setIssues([]);

    async function run(): Promise<void> {
      // 本期没有扫描来源的分类（整合包）直接给出空状态。
      if (kinds.length === 0) {
        if (!cancelled) {
          setState("empty");
          setReason("noSource");
        }
        return;
      }

      const resolved = await resolveResourceRoot(targetRoot && targetVersion ? { root: targetRoot, versionId: targetVersion } : undefined);
      if (cancelled) return;
      setRoot(resolved);
      if (!resolved) {
        setState("empty");
        setReason("noRoot");
        return;
      }

      const results = await Promise.allSettled(
        kinds.map((kind) => scanKind(resolved.dir, kind))
      );
      if (cancelled) return;

      const collected: ResourceEntry[] = [];
      const failures: string[] = [];
      const nextCounts: Partial<Record<ResourceKind, number>> = {};

      kinds.forEach((kind, index) => {
        const result = results[index];
        if (!result) return;
        if (result.status === "fulfilled") {
          nextCounts[kind] = result.value.length;
          collected.push(...result.value);
        } else {
          failures.push(describeScanError(result.reason, kind, resolved.dir));
        }
      });

      setCounts((prev) => ({ ...prev, ...nextCounts }));

      // 失败原因始终保留：错误态用它做详情，部分失败时用顶部提醒条展示。
      setIssues(failures);

      if (collected.length === 0) {
        // 全部种类都失败 → 明确报错；部分成功但没有条目 → 该分类确实为空。
        const allFailed = failures.length === kinds.length;
        setState(allFailed ? "error" : "empty");
        setReason(allFailed ? null : "noItems");
        return;
      }

      const order = new Map(kinds.map((kind, index) => [kind, index]));
      collected.sort(compareByKindThenName(order));
      setEntries(collected);
      setState("ready");
    }

    void run().catch((error: unknown) => {
      if (cancelled) return;
      setState("error");
      setIssues([error instanceof Error ? error.message : String(error)]);
    });
    return () => {
      cancelled = true;
    };
  }, [category, nonce, targetRoot, targetVersion]);

  return { state, reason, entries, issues, root, counts, reload };
}
