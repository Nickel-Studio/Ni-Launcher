import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { setRoute } from "../../../lib/nav";
import { useSettings } from "../../../lib/settings";
import { failureLabel } from "../../../lib/failure";
import { useDownloadCenter } from "../../download/store";
import { IconFolderOpen } from "../../resources/icons";
import modPlaceholder from "../../../assets/mod-placeholder.png";
import {
  Button,
  IconButton,
  IconCheck,
  IconClose,
  IconDownload,
  IconLayers,
  IconMinus,
  IconPlus,
  IconRefresh,
  IconTrash,
  LoadingCard,
  Modal,
  SegmentedControl,
} from "../../ui";
import { confirmDialog } from "../dialog";
import { describeError } from "./api";
import {
  actOnVersionMods,
  fetchVersionModIcon,
  modDisplayName,
  modFileName,
  type ModAction,
  type ModEntry,
} from "./mods";
import { useMods } from "./useMods";
import "./ModsPanel.css";

interface ModsPanelProps {
  root: string;
  versionId: string;
  displayName: string;
  onBrowse: () => void;
  onSettings: () => void;
}

type ModFilter = "all" | "enabled" | "disabled" | "updates" | "duplicate";

// 图标缓存按「根目录 + 实例 + 文件 + JAR 内条目」区分；失败也缓存为 null，
// 避免同一张坏图标在筛选/重渲染时反复请求后端。
const iconCache = new Map<string, Promise<string | null>>();
const ICON_CONCURRENCY = 4;
let iconRunning = 0;
const iconQueue: Array<() => void> = [];

/** 限制并发，避免一次渲染 300 条列表时同时打开 300 个 JAR。 */
async function scheduleIcon(
  read: () => Promise<string | null>
): Promise<string | null> {
  if (iconRunning >= ICON_CONCURRENCY) {
    await new Promise<void>((resolve) => iconQueue.push(resolve));
  }
  iconRunning += 1;
  try {
    return await read();
  } finally {
    iconRunning -= 1;
    iconQueue.shift()?.();
  }
}

function loadModIcon(
  root: string,
  versionId: string,
  path: string,
  iconPath: string
): Promise<string | null> {
  const key = `${root}\u0000${versionId}\u0000${path}\u0000${iconPath}`;
  const cached = iconCache.get(key);
  if (cached) return cached;
  const pending = scheduleIcon(() =>
    fetchVersionModIcon(root, versionId, path, iconPath)
  ).catch(() => null);
  iconCache.set(key, pending);
  return pending;
}

function ModIcon({
  root,
  versionId,
  entry,
}: {
  root: string;
  versionId: string;
  entry: ModEntry;
}) {
  const { path, iconPath } = entry;
  const host = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  // 只有接近视口的条目才请求图标；300 条列表不会一次打开 300 个 JAR。
  useEffect(() => {
    if (!iconPath) return;
    const node = host.current;
    if (!node || typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      (records) => {
        if (records.some((record) => record.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "200px 0px" }
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [iconPath]);

  useEffect(() => {
    let alive = true;
    setSrc(null);
    setFailed(false);
    if (visible && iconPath) {
      void loadModIcon(root, versionId, path, iconPath).then((value) => {
        if (alive && value) setSrc(value);
      });
    }
    return () => {
      alive = false;
    };
  }, [visible, root, versionId, path, iconPath]);
  // 占位图始终透明无框：真实图标迟到或失败都只回退到同一张图。
  const image = src && !failed ? src : modPlaceholder;
  return (
    <span ref={host} className="vst-mod__icon" aria-hidden="true">
      <img
        src={image}
        alt=""
        loading="lazy"
        decoding="async"
        data-placeholder={image === modPlaceholder ? "true" : undefined}
        onError={() => {
          if (src && !failed) setFailed(true);
        }}
      />
    </span>
  );
}

function ModInfoIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v6m0-10v.1" />
    </svg>
  );
}

export function ModsPanel({
  root,
  versionId,
  displayName,
  onBrowse,
  onSettings,
}: ModsPanelProps) {
  const { settings: appSettings } = useSettings();
  const center = useDownloadCenter();
  const {
    entries,
    loading,
    checkingUpdates,
    scanError,
    updatesDisabled,
    reload,
    stopUpdateCheck,
  } = useMods(root, versionId);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ModFilter>("all");
  const [busy, setBusy] = useState<ModAction | null>(null);
  const [error, setError] = useState("");
  const [detail, setDetail] = useState<ModEntry | null>(null);
  const [updateTaskId, setUpdateTaskId] = useState<string | null>(null);
  const mounted = useRef(true);
  const session = useRef(0);

  useEffect(() => {
    mounted.current = true;
    setSelected(new Set());
    setDetail(null);
    setBusy(null);
    setError("");
    setUpdateTaskId(null);
    return () => {
      mounted.current = false;
      session.current += 1;
    };
  }, [root, versionId]);

  useEffect(() => {
    setSelected(
      (previous) =>
        new Set(
          entries
            .filter((entry) => previous.has(entry.path))
            .map((entry) => entry.path)
        )
    );
  }, [entries]);

  const updateTask = center.tasks.find(
    (task) => task.task?.taskId === updateTaskId
  );
  useEffect(() => {
    if (!updateTaskId || !updateTask || updateTask.status === "running") return;
    setUpdateTaskId(null);
    if (updateTask.status === "done") {
      setSelected(new Set());
      void reload();
    } else if (updateTask.status === "error") {
      setError(
        updateTask.message || "Mod 更新失败，请在下载中心查看详情后重试"
      );
    }
  }, [updateTaskId, updateTask, reload]);

  const visible = useMemo(() => {
    const search = query.trim().toLocaleLowerCase();
    return entries.filter((entry) => {
      if (filter === "enabled" && !entry.enabled) return false;
      if (filter === "disabled" && entry.enabled) return false;
      if (filter === "updates" && !entry.updateAvailable) return false;
      if (filter === "duplicate" && !entry.duplicateGroup) return false;
      return (
        !search ||
        [
          entry.name,
          entry.description,
          entry.version,
          ...entry.physicalPaths.map(modFileName),
          ...entry.tags,
        ].some((text) => text.toLocaleLowerCase().includes(search))
      );
    });
  }, [entries, query, filter]);
  const duplicateGroupCount = useMemo(
    () =>
      new Set(
        entries.flatMap((entry) =>
          entry.duplicateGroup ? [entry.duplicateGroup] : []
        )
      ).size,
    [entries]
  );
  const detailDuplicateCount = detail?.duplicateGroup
    ? entries.filter((entry) => entry.duplicateGroup === detail.duplicateGroup)
        .length
    : 0;
  const chosen = entries.filter((entry) => selected.has(entry.path));
  // 批量操作窗的退场不能靠条件渲染瞬间卸载：先播 180ms 动画再移除。
  const [selectionMounted, setSelectionMounted] = useState(false);
  const [selectionLeaving, setSelectionLeaving] = useState(false);
  const [selectionCount, setSelectionCount] = useState(0);
  useEffect(() => {
    if (chosen.length > 0) {
      setSelectionMounted(true);
      setSelectionLeaving(false);
      setSelectionCount(chosen.length);
      return;
    }
    if (!selectionMounted) return;
    setSelectionLeaving(true);
    const timer = window.setTimeout(() => {
      setSelectionMounted(false);
      setSelectionLeaving(false);
    }, 180);
    return () => window.clearTimeout(timer);
  }, [chosen.length, selectionMounted]);
  const working = busy !== null || updateTask?.status === "running";
  const allVisibleSelected =
    visible.length > 0 && visible.every((entry) => selected.has(entry.path));

  const toggleSelection = (path: string) =>
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  const selectVisible = () =>
    setSelected((previous) => {
      const next = new Set(previous);
      for (const entry of visible) {
        if (allVisibleSelected) next.delete(entry.path);
        else next.add(entry.path);
      }
      return next;
    });

  const act = async (action: ModAction, targets: ModEntry[] = chosen) => {
    if (working || loading) return;
    const current = session.current;
    const isCurrent = () => mounted.current && session.current === current;
    const matching = targets.filter((entry) =>
      action === "enable"
        ? !entry.enabled
        : action === "disable"
        ? entry.enabled
        : action === "update"
        ? entry.updateAvailable
        : true
    );
    if (action !== "install" && matching.length === 0) return;
    if (action === "update" && (updatesDisabled !== false || checkingUpdates))
      return;
    const deletePaths = [
      ...new Set(
        matching.flatMap((entry) =>
          entry.physicalPaths.length > 0 ? entry.physicalPaths : [entry.path]
        )
      ),
    ];
    setBusy(action);
    setError("");
    try {
      if (
        action === "delete" &&
        !(await confirmDialog(
          `删除 ${deletePaths.length} 个物理文件？\n${deletePaths
            .map(modFileName)
            .join("\n")}`,
          "删除 Mod"
        ))
      )
        return;
      if (!isCurrent()) return;
      stopUpdateCheck();
      const taskId = await actOnVersionMods(
        root,
        versionId,
        action === "delete" ? deletePaths : matching.map((entry) => entry.path),
        action,
        appSettings.fileDownloadSource,
        displayName
      );
      if (!isCurrent()) return;
      if (action === "update" && taskId !== null) setUpdateTaskId(taskId);
      else await reload();
    } catch (err) {
      if (isCurrent()) setError(describeError(err));
    } finally {
      if (isCurrent()) setBusy(null);
    }
  };

  const openFolder = async () => {
    const current = session.current;
    try {
      await invoke("open_folder", { dir: root, versionId, which: "mods" });
    } catch (err) {
      if (mounted.current && session.current === current)
        setError(describeError(err));
    }
  };

  return (
    <div className="vst-mods">
      <input
        className="vst-input vst-mods__search"
        type="search"
        aria-label="搜索 Mod"
        placeholder="搜索 Mod 名称、描述、版本、文件名或标签"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <div className="vst-mods__toolbar">
        <Button
          size="sm"
          iconLeft={<IconFolderOpen />}
          onClick={() => void openFolder()}
        >
          打开文件夹
        </Button>
        <Button
          size="sm"
          iconLeft={<IconPlus />}
          disabled={working || loading}
          loading={busy === "install"}
          onClick={() => void act("install")}
        >
          从文件安装
        </Button>
        <Button size="sm" iconLeft={<IconDownload />} onClick={onBrowse}>
          下载新 Mod
        </Button>
        <Button
          size="sm"
          disabled={working || visible.length === 0}
          onClick={selectVisible}
        >
          {allVisibleSelected ? "取消全选" : "全选"}
        </Button>
        <IconButton
          size="sm"
          label="重新扫描 Mod"
          disabled={working || loading}
          onClick={() => void reload()}
        >
          <IconRefresh />
        </IconButton>
      </div>
      {error || (scanError && entries.length > 0) ? (
        <div className="vst-alert vst-mods__alert" role="alert">
          <span>
            {[error, entries.length > 0 ? scanError : ""]
              .filter(Boolean)
              .join("；")}
          </span>
          <Button
            size="sm"
            disabled={working || loading}
            onClick={() => {
              setError("");
              void reload();
            }}
          >
            重新扫描
          </Button>
        </div>
      ) : null}
      {updatesDisabled ? (
        <div className="vst-mods__notice">
          已在实例设置中关闭 Mod 更新
          <Button size="sm" variant="ghost" onClick={onSettings}>
            修改设置
          </Button>
        </div>
      ) : null}
      {updateTask?.status === "running" ? (
        <div className="vst-mods__notice" role="status">
          {updateTask.stage || "正在更新 Mod"}
          <Button size="sm" onClick={() => setRoute("downloads")}>
            查看下载
          </Button>
        </div>
      ) : null}
      <section
        className="vst-mods__list"
        aria-label="Mod 列表"
        aria-busy={loading}
      >
        <div role="group" aria-label="Mod 筛选">
          <SegmentedControl
            value={filter}
            onChange={(value) => setFilter(value as ModFilter)}
            options={[
              { value: "all", label: `全部 ${entries.length}` },
              {
                value: "enabled",
                label: `启用 ${
                  entries.filter((entry) => entry.enabled).length
                }`,
              },
              {
                value: "disabled",
                label: `禁用 ${
                  entries.filter((entry) => !entry.enabled).length
                }`,
              },
              {
                value: "updates",
                label: `可更新 ${
                  entries.filter((entry) => entry.updateAvailable).length
                }`,
              },
              {
                value: "duplicate",
                label: `重复 ${duplicateGroupCount}`,
              },
            ]}
          />
        </div>
        {loading && entries.length === 0 ? (
          <LoadingCard label="正在扫描 Mod..." />
        ) : scanError && entries.length === 0 ? (
          <LoadingCard
            state="error"
            label={failureLabel(scanError)}
            action={
              <Button
                size="sm"
                variant="secondary"
                iconLeft={<IconRefresh />}
                disabled={working || loading}
                onClick={() => {
                  setError("");
                  void reload();
                }}
              >
                重新扫描
              </Button>
            }
          />
        ) : visible.length === 0 ? (
          <p className="vst-mods__empty">
            {query || filter !== "all"
              ? "没有符合条件的 Mod"
              : "还没有安装 Mod，选择本地文件或前往资源页下载"}
          </p>
        ) : (
          <ul className="vst-mods__entries">
            {visible.map((entry) => (
              <li
                key={entry.path}
                className="vst-mod"
                data-path={entry.path}
                data-selected={selected.has(entry.path)}
                data-enabled={entry.enabled}
              >
                <button
                  type="button"
                  className="vst-mod__select"
                  role="checkbox"
                  aria-checked={selected.has(entry.path)}
                  aria-label={`选择 ${entry.name}`}
                  disabled={working || loading}
                  onClick={() => toggleSelection(entry.path)}
                >
                  <ModIcon root={root} versionId={versionId} entry={entry} />
                  <span
                    className="vst-mod__text"
                    data-meta={entry.metaLoaded ? "true" : undefined}
                  >
                    <span className="vst-mod__name">
                      {entry.name}
                      <span className="vst-mod__version">{entry.version}</span>
                      {!entry.enabled ? (
                        <span className="vst-badge">已禁用</span>
                      ) : null}
                      {entry.updateAvailable ? (
                        <span className="vst-badge">可更新</span>
                      ) : null}
                      {entry.duplicateGroup ? (
                        <span className="vst-badge">重复</span>
                      ) : null}
                      {entry.physicalPaths.length > 1 ? (
                        <span className="vst-badge">
                          {entry.physicalPaths.length} 个文件
                        </span>
                      ) : null}
                    </span>
                    <span className="vst-mod__description">
                      {entry.description || modDisplayName(entry.path)}
                    </span>
                  </span>
                </button>
                <div className="vst-mod__actions">
                  <IconButton
                    size="sm"
                    label={`查看 ${entry.name} 信息`}
                    onClick={() => setDetail(entry)}
                  >
                    <ModInfoIcon />
                  </IconButton>
                  <IconButton
                    size="sm"
                    label={`${entry.enabled ? "禁用" : "启用"} ${entry.name}`}
                    disabled={working || loading}
                    onClick={() =>
                      void act(entry.enabled ? "disable" : "enable", [entry])
                    }
                  >
                    {entry.enabled ? <IconMinus /> : <IconCheck />}
                  </IconButton>
                  <IconButton
                    size="sm"
                    label={`删除 ${entry.name}`}
                    disabled={working || loading}
                    onClick={() => void act("delete", [entry])}
                  >
                    <IconTrash />
                  </IconButton>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      {selectionMounted ? (
        <div
          className="vst-mods__selection"
          data-leaving={selectionLeaving ? "true" : undefined}
          aria-hidden={selectionLeaving ? true : undefined}
          role="region"
          aria-label="Mod 批量操作"
        >
          <span className="vst-mods__selection-count">
            已选择 {selectionCount} 个 Mod
          </span>
          <div className="vst-mods__toolbar">
            <Button
              size="sm"
              iconLeft={<IconLayers />}
              disabled={working || loading}
              onClick={() => void act("copy")}
            >
              复制
            </Button>
            <Button
              size="sm"
              iconLeft={<IconRefresh />}
              disabled={
                working ||
                loading ||
                checkingUpdates ||
                updatesDisabled !== false ||
                !chosen.some((entry) => entry.updateAvailable)
              }
              onClick={() => void act("update")}
            >
              更新
            </Button>
            <Button
              size="sm"
              iconLeft={<IconCheck />}
              disabled={
                working || loading || chosen.every((entry) => entry.enabled)
              }
              onClick={() => void act("enable")}
            >
              启用
            </Button>
            <Button
              size="sm"
              iconLeft={<IconMinus />}
              disabled={
                working || loading || chosen.every((entry) => !entry.enabled)
              }
              onClick={() => void act("disable")}
            >
              禁用
            </Button>
            <Button
              size="sm"
              variant="danger"
              iconLeft={<IconTrash />}
              disabled={working || loading}
              onClick={() => void act("delete")}
            >
              删除
            </Button>
            <IconButton
              size="sm"
              label="取消选择"
              disabled={working}
              onClick={() => setSelected(new Set())}
            >
              <IconClose />
            </IconButton>
          </div>
        </div>
      ) : null}
      <Modal
        open={detail !== null}
        onClose={() => setDetail(null)}
        title={detail?.name}
        ariaLabel="Mod 信息"
      >
        {detail ? (
          <dl className="vst-mods__metadata">
            <dt>版本</dt>
            <dd>{detail.version || "未提供版本号"}</dd>
            <dt>状态</dt>
            <dd>
              {detail.enabled ? "已启用" : "已禁用"}
              {detail.updateAvailable ? " · 可更新" : ""}
            </dd>
            <dt>物理文件</dt>
            <dd className="vst-mods__paths nw-mono">
              {detail.physicalPaths.map((path) => (
                <span key={path}>{path}</span>
              ))}
            </dd>
            <dt>路径</dt>
            <dd className="nw-mono">{detail.path}</dd>
            {detail.tags.length > 0 ? (
              <>
                <dt>标签</dt>
                <dd>{detail.tags.join(" · ")}</dd>
              </>
            ) : null}
            {detail.duplicateGroup ? (
              <>
                <dt>重复组</dt>
                <dd>{detailDuplicateCount} 个逻辑 Mod 使用同一身份</dd>
              </>
            ) : null}
            {detail.description ? (
              <>
                <dt>描述</dt>
                <dd>{detail.description}</dd>
              </>
            ) : null}
          </dl>
        ) : null}
      </Modal>
    </div>
  );
}
