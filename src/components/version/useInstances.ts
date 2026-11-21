import { useCallback, useEffect, useState } from "react";
import { isTauri } from "../../lib/settings";
import {
  getLaunchInstance,
  getVersionFocus,
  setLaunchInstance,
  setVersionFocus,
} from "../../lib/instances";
import { loadGameSettings, saveGameSettings } from "./gameSettings";
import {
  describeInstancesError,
  ensureDefaultInstance,
  fetchDefaultInstanceDir,
  instanceRootIdentity,
  loadRegistry,
  orderInstanceRoots,
  renameInstanceRoot,
  resolveInstanceRoot,
  saveRegistry,
  scanInstances,
  type InstanceEntry,
} from "./instancesApi";

/** 实例扫描状态机。`idle` = 尚未选定实例目录。 */
export type ScanState = "idle" | "loading" | "ready" | "empty" | "error";

export interface UseInstanceRegistry {
  /** 已注册的 `.minecraft` 根目录。 */
  roots: string[];
  labels: Record<string, string>;
  /** 启动器自带 / 便携包定义的默认游戏目录；展示为「默认实例目录」。 */
  defaultDir: string;
  /** 当前激活目录（= game.json 的 gameDir）。 */
  active: string;
  /** 首次读取/初始化中。 */
  loading: boolean;
  /** 正在写入（激活 / 新增 / 移除），用于禁用交互而不是掩盖结果。 */
  busy: boolean;
  error: string;
  reload: () => void;
  activate: (dir: string) => Promise<void>;
  addRoot: (dir: string) => Promise<void>;
  renameRoot: (dir: string, label: string) => Promise<void>;
  removeRoot: (dir: string) => Promise<void>;
}

/** 去尾部路径分隔符，使比较与存储用同一形式。 */
function normalizeDir(dir: string): string {
  return dir.trim().replace(/[\\/]+$/, "");
}

/**
 * 实例根目录注册表。
 *
 * 关键不变量：**激活目录 = `game.json` 的 `gameDir`**，
 * 注册表里的 `active` 只是它的镜像；两边任何不一致都在这里被纠正。
 */
export function useInstanceRegistry(): UseInstanceRegistry {
  const [roots, setRoots] = useState<string[]>([]);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const [defaultDir, setDefaultDir] = useState("");
  const [active, setActive] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");

    async function bootstrap(): Promise<void> {
      if (!isTauri()) throw new Error("未连接到启动器后端");
      const game = await loadGameSettings();
      const defaultInstanceDir = await fetchDefaultInstanceDir().catch(
        () => ""
      );
      let registry = await loadRegistry();
      if (registry.roots.length === 0) {
        const created = await ensureDefaultInstance();
        registry = await loadRegistry();
        if (!registry.active && created) registry.active = created;
      }
      let nextRoots = [...registry.roots];
      let nextActive = resolveInstanceRoot(
        nextRoots,
        normalizeDir(game.gameDir) || registry.active || nextRoots[0] || ""
      );
      if (nextActive && !nextRoots.includes(nextActive))
        nextRoots.push(nextActive);
      nextRoots = orderInstanceRoots(nextRoots, defaultInstanceDir);
      nextActive =
        resolveInstanceRoot(nextRoots, nextActive) || nextRoots[0] || "";
      if (nextActive && normalizeDir(game.gameDir) !== nextActive) {
        const saved = await saveGameSettings({ ...game, gameDir: nextActive });
        if (!saved.persisted) {
          throw new Error(
            "无法把当前实例目录写入游戏设置，请检查配置目录权限后重试"
          );
        }
      }
      if (
        nextActive !== registry.active ||
        nextRoots.join("\n") !== registry.roots.join("\n")
      ) {
        await saveRegistry({
          ...registry,
          roots: nextRoots,
          active: nextActive,
        });
      }
      if (cancelled) return;
      setRoots(nextRoots);
      setLabels(registry.labels);
      setDefaultDir(resolveInstanceRoot(nextRoots, defaultInstanceDir));
      setActive(nextActive);
    }

    void bootstrap()
      .catch((err: unknown) => {
        if (!cancelled) setError(describeInstancesError(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [nonce]);

  /** 改写「激活目录」：先写 game.json（单一真相），再同步注册表镜像。 */
  const writeActive = useCallback(
    async (dir: string): Promise<{ roots: string[]; active: string }> => {
      const requested = normalizeDir(dir);
      if (!requested) throw new Error("实例目录不能为空");
      let target = resolveInstanceRoot(roots, requested);
      const game = await loadGameSettings();
      const saved = await saveGameSettings({ ...game, gameDir: target });
      if (!saved.persisted)
        throw new Error("无法写入游戏设置（game.json），激活未生效");
      // Re-read after every settings write so concurrent additions are retained.
      let registry = await loadRegistry();
      let nextActive = resolveInstanceRoot(registry.roots, target);
      while (nextActive !== target) {
        const currentGame = await loadGameSettings();
        const aligned = await saveGameSettings({
          ...currentGame,
          gameDir: nextActive,
        });
        if (!aligned.persisted)
          throw new Error("无法写入游戏设置（game.json），激活未生效");
        target = nextActive;
        registry = await loadRegistry();
        nextActive = resolveInstanceRoot(registry.roots, target);
      }
      const nextRoots = orderInstanceRoots(
        registry.roots.includes(nextActive)
          ? registry.roots
          : [...registry.roots, nextActive],
        defaultDir
      );
      await saveRegistry({ ...registry, roots: nextRoots, active: nextActive });
      return { roots: nextRoots, active: nextActive };
    },
    [roots, defaultDir]
  );

  const activate = useCallback(
    async (dir: string) => {
      setBusy(true);
      setError("");
      try {
        const next = await writeActive(dir);
        setRoots(next.roots);
        setActive(next.active);
      } catch (err) {
        setError(describeInstancesError(err));
        throw err;
      } finally {
        setBusy(false);
      }
    },
    [writeActive]
  );

  const addRoot = useCallback(
    async (dir: string) => {
      setBusy(true);
      setError("");
      try {
        // 已在列表中的目录不重复添加，直接切为激活目录。
        const next = await writeActive(dir);
        setRoots(next.roots);
        setActive(next.active);
      } catch (err) {
        setError(describeInstancesError(err));
        throw err;
      } finally {
        setBusy(false);
      }
    },
    [writeActive]
  );

  const renameRoot = useCallback(async (dir: string, label: string) => {
    setBusy(true);
    setError("");
    try {
      const target = normalizeDir(dir);
      const next = label.trim();
      await renameInstanceRoot(target, next);
      setLabels((previous) => ({ ...previous, [target]: next }));
    } catch (err) {
      setError(describeInstancesError(err));
      throw err;
    } finally {
      setBusy(false);
    }
  }, []);

  const removeRoot = useCallback(
    async (dir: string) => {
      setBusy(true);
      setError("");
      try {
        const target = normalizeDir(dir);
        const registry = await loadRegistry();
        const nextRoots = orderInstanceRoots(
          registry.roots.filter(
            (item) =>
              instanceRootIdentity(item) !== instanceRootIdentity(target)
          ),
          defaultDir
        );
        const currentActive = resolveInstanceRoot(
          registry.roots,
          active || registry.active
        );
        const nextActive =
          instanceRootIdentity(currentActive) === instanceRootIdentity(target)
            ? nextRoots[0] ?? ""
            : resolveInstanceRoot(nextRoots, currentActive) ||
              nextRoots[0] ||
              "";
        await saveRegistry({
          ...registry,
          roots: nextRoots,
          active: nextActive,
        });

        if (nextActive !== active) {
          const game = await loadGameSettings();
          const saved = await saveGameSettings({
            ...game,
            gameDir: nextActive,
          });
          if (!saved.persisted)
            throw new Error("无法写入游戏设置（game.json），移除未生效");
        }

        // 启动实例不能指向已被移除的目录，否则会出现「目标目录已不在列表中却仍是启动实例」。
        const launch = getLaunchInstance();
        if (
          launch &&
          instanceRootIdentity(launch.root) === instanceRootIdentity(target)
        )
          setLaunchInstance(null);
        // 版本焦点同理：目录已移除时焦点失效，一并清空避免版本设置页读到悬空值。
        const focus = getVersionFocus();
        if (
          focus &&
          instanceRootIdentity(focus.root) === instanceRootIdentity(target)
        )
          setVersionFocus(null);

        setRoots(nextRoots);
        setActive(nextActive);
      } catch (err) {
        setError(describeInstancesError(err));
        throw err;
      } finally {
        setBusy(false);
      }
    },
    [active, defaultDir]
  );

  return {
    roots,
    labels,
    defaultDir: resolveInstanceRoot(roots, defaultDir),
    active,
    loading,
    busy,
    error,
    reload,
    activate,
    addRoot,
    renameRoot,
    removeRoot,
  };
}

export interface UseInstanceScan {
  state: ScanState;
  entries: InstanceEntry[];
  error: string;
  /** 这批结果真正来自哪个目录（避免目录切换动画期间把旧结果写到新目录上）。 */
  scannedDir: string;
  reload: () => void;
}

/** 扫描某个 `.minecraft` 根目录下的版本实例。 */
export function useInstanceScan(dir: string): UseInstanceScan {
  const [state, setState] = useState<ScanState>("idle");
  const [entries, setEntries] = useState<InstanceEntry[]>([]);
  const [error, setError] = useState("");
  const [scannedDir, setScannedDir] = useState("");
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    window.addEventListener("nickel:instances-changed", reload);
    return () =>
      window.removeEventListener("nickel:instances-changed", reload);
  }, [reload]);

  useEffect(() => {
    if (!dir) {
      setState("idle");
      setEntries([]);
      setError("");
      setScannedDir("");
      return;
    }

    let cancelled = false;
    setState("loading");
    setError("");
    setEntries([]);

    scanInstances(dir)
      .then((list) => {
        if (cancelled) return;
        setEntries(list);
        setScannedDir(dir);
        setState(list.length > 0 ? "ready" : "empty");
        const selected = getLaunchInstance();
        const entry =
          selected?.root === dir
            ? list.find((item) => item.id === selected.versionId)
            : undefined;
        if (
          selected &&
          entry &&
          (selected.displayName !== entry.displayName ||
            selected.loader !== entry.loader ||
            selected.icon !== entry.icon)
        ) {
          setLaunchInstance({
            ...selected,
            displayName: entry.displayName,
            loader: entry.loader,
            icon: entry.icon,
          });
        }
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setEntries([]);
        setScannedDir(dir);
        setError(describeInstancesError(err));
        setState("error");
      });

    return () => {
      cancelled = true;
    };
  }, [dir, nonce]);

  return { state, entries, error, scannedDir, reload };
}
