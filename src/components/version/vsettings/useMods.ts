import { useCallback, useEffect, useRef, useState } from "react";
import { loadGameSettings } from "../gameSettings";
import { describeError, fetchVersionSettings } from "./api";
import {
  fetchVersionModList,
  fetchVersionModMeta,
  fetchVersionModUpdates,
  modUpdatesDisabled,
  type ModEntry,
  type ModMeta,
} from "./mods";

// 首屏只做 read_dir，10 秒只是兜底；正常 300+ Mod 应远早于此返回。
export const MOD_LIST_TIMEOUT_MS = 10_000;
// 单条元数据只解压一个清单，10 秒后静默放弃并保留占位。
export const MOD_META_TIMEOUT_MS = 10_000;
// 读取实例/全局更新设置；不阻塞首屏列表与元数据队列。
export const MOD_POLICY_TIMEOUT_MS = 30_000;
// 后端更新查询总预算为 30 秒，另留 IPC 返回时间。
export const MOD_UPDATE_TIMEOUT_MS = 45_000;
// 元数据队列并发：与图标并发一致，避免同时打开过多 JAR。
const META_CONCURRENCY = 4;

async function bounded<T>(
  work: Promise<T>,
  timeoutMs: number,
  message: string
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

/** 统一去掉末尾句号，包装文案只补一次标点，避免出现“。。”。 */
function describe(error: unknown): string {
  return describeError(error).replace(/[。.]+$/u, "");
}

/**
 * 限并发执行；worker 自己吞掉单条失败，队列不会因一条 Mod 中断。
 * shouldContinue 在每次取任务前检查，实例切换后旧队列不再发起新请求。
 */
async function runLimited<T>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<void>,
  shouldContinue: () => boolean = () => true
): Promise<void> {
  let next = 0;
  const runners = Array.from(
    { length: Math.min(limit, items.length) },
    async () => {
      while (true) {
        if (!shouldContinue()) return;
        const index = next;
        next += 1;
        if (index >= items.length) return;
        await worker(items[index]);
      }
    }
  );
  await Promise.all(runners);
}

/** 身份键按 path 暂存；只有两个以上条目共享同一键时才显示“重复”。 */
function withDuplicateGroups(
  entries: ModEntry[],
  keys: Map<string, string>
): ModEntry[] {
  const counts = new Map<string, number>();
  for (const entry of entries) {
    const key = keys.get(entry.path);
    if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return entries.map((entry) => {
    const key = keys.get(entry.path);
    const group = key && (counts.get(key) ?? 0) > 1 ? key : null;
    return entry.duplicateGroup === group
      ? entry
      : { ...entry, duplicateGroup: group };
  });
}

function mergeMeta(entry: ModEntry, meta: ModMeta): ModEntry {
  return {
    ...entry,
    name: meta.name || entry.name,
    description: meta.description || entry.description,
    version: meta.version,
    tags: meta.tags,
    iconPath: meta.iconPath,
    metaLoaded: true,
  };
}

/**
 * 首屏先出文件名占位列表，再按并发队列逐条补齐元数据，最后后台检查更新。
 * 所有合并都按 path 原位替换，绝不重排或重载整张列表。
 */
export function useMods(root: string, versionId: string) {
  const [entries, setEntries] = useState<ModEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [scanError, setScanError] = useState("");
  const [updatesDisabled, setUpdatesDisabled] = useState<boolean | null>(null);
  const request = useRef(0);
  const updateRequest = useRef(0);
  const mounted = useRef(false);
  // 元数据身份键与列表展示分离：候选键可以暂存，展示只在重复时出现。
  const identityKeys = useRef(new Map<string, string>());

  const stopUpdateCheck = useCallback(() => {
    updateRequest.current += 1;
    setCheckingUpdates(false);
  }, []);

  const reload = useCallback(async () => {
    const current = ++request.current;
    const update = ++updateRequest.current;
    const isCurrent = () => mounted.current && current === request.current;
    const canUpdate = () => isCurrent() && update === updateRequest.current;
    setLoading(true);
    setCheckingUpdates(false);
    setScanError("");
    setUpdatesDisabled(null);

    const local = bounded(
      fetchVersionModList(root, versionId),
      MOD_LIST_TIMEOUT_MS,
      "读取 Mod 列表超时，请确认实例目录可访问后重新扫描"
    )
      .then((mods) => {
        if (!isCurrent()) return mods;
        identityKeys.current.clear();
        for (const entry of mods) {
          if (entry.duplicateGroup) {
            identityKeys.current.set(entry.path, entry.duplicateGroup);
          }
        }
        setEntries(withDuplicateGroups(mods, identityKeys.current));
        return mods;
      })
      .catch((error: unknown) => {
        if (isCurrent())
          setScanError(
            `扫描 Mod 失败：${describe(error)}。请检查 Mod 文件夹后重新扫描。`
          );
        return null;
      })
      .finally(() => {
        if (isCurrent()) setLoading(false);
      });

    const policy = bounded(
      Promise.all([fetchVersionSettings(root, versionId), loadGameSettings()]),
      MOD_POLICY_TIMEOUT_MS,
      "读取 Mod 更新设置超时，请重新扫描"
    )
      .then(([localSettings, globalSettings]) => {
        const disabled = modUpdatesDisabled(localSettings, globalSettings);
        if (isCurrent()) setUpdatesDisabled(disabled);
        return disabled;
      })
      .catch(() => {
        // 更新开关读取失败只意味着本轮不检查更新，不写页面错误。
        return null;
      });

    void local
      .then(async (mods) => {
        if (!mods || !isCurrent()) return;
        await runLimited(mods, META_CONCURRENCY, async (entry) => {
          if (!isCurrent()) return;
          try {
            const meta = await bounded(
              fetchVersionModMeta(
                root,
                versionId,
                entry.path,
                entry.physicalPaths
              ),
              MOD_META_TIMEOUT_MS,
              "读取 Mod 信息超时"
            );
            if (!isCurrent() || !meta) return;
            if (meta.duplicateGroup) {
              identityKeys.current.set(entry.path, meta.duplicateGroup);
            } else {
              identityKeys.current.delete(entry.path);
            }
            setEntries((previous) =>
              withDuplicateGroups(
                previous.map((item) =>
                  item.path === entry.path ? mergeMeta(item, meta) : item
                ),
                identityKeys.current
              )
            );
          } catch {
            // 单条失败静默保留占位，不写 scanError，也不阻断其它条目。
          }
        });

        // 元数据全部完成后才进入更新检查，避免 300 个 JAR 同时解压与联网。
        const disabled = await policy;
        if (!canUpdate() || mods.length === 0 || disabled !== false) return;
        setCheckingUpdates(true);
        try {
          const checked = await bounded(
            fetchVersionModUpdates(
              root,
              versionId,
              mods.map((entry) => entry.path)
            ),
            MOD_UPDATE_TIMEOUT_MS,
            "Mod 更新检查超时，请检查网络后重新扫描"
          );
          if (!canUpdate()) return;
          const byPath = new Map(checked.map((item) => [item.path, item]));
          setEntries((previous) =>
            withDuplicateGroups(
              previous.map((entry) => {
                const updateAvailable =
                  byPath.get(entry.path)?.updateAvailable ?? false;
                // 任何失败或缺失都静默视为无更新，不产生行内错误。
                return entry.updateAvailable === updateAvailable
                  ? entry
                  : { ...entry, updateAvailable };
              }),
              identityKeys.current
            )
          );
        } catch {
          // 尽力而为：超时或网络失败都不提示，保留本地列表。
        } finally {
          if (canUpdate()) setCheckingUpdates(false);
        }
      })
      .catch(() => {
        // 队列自身不应产生整页错误；列表与设置错误已分别收敛。
      });

    // 文件动作等待首屏列表即可，不继续等待元数据与联网检查。
    await local;
  }, [root, versionId]);

  useEffect(() => {
    mounted.current = true;
    setEntries([]);
    identityKeys.current.clear();
    void reload();
    return () => {
      mounted.current = false;
      request.current += 1;
      updateRequest.current += 1;
    };
  }, [reload]);

  return {
    entries,
    loading,
    checkingUpdates,
    scanError,
    updatesDisabled,
    reload,
    stopUpdateCheck,
  };
}
