/**
 * 原版游戏 · 线上版本下载页。
 *
 * 两段式视图：
 * 1. 版本列表：最新版本卡（正式版 / 预览版）+ 四个折叠分组
 *    （正式版 / 预览版 / 远古版 / 愚人节版，归类逻辑见 versionGroups.ts）
 *    + 搜索框（按版本 id 过滤）；
 * 2. 加载器步骤（返回箭头）：Forge / NeoForge / Fabric / OptiFine 四行
 *    （原位展开的完整版本卡片）→「开始下载」到命名实例。
 *
 * 数据全部来自后端真实命令：`list_remote_versions` / `list_loaders` /
 * `install_version`（进度经 install-event 自动接入 download store，
 * 右下角气泡与下载详情页无需本页关心）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Collapse, LoadingCard } from "../ui";
import { failureLabel } from "../../lib/failure";
import { IconChevronDown } from "../ui/icons";
import { IconSearch } from "../modrinth/icons";
import { LoaderPicker } from "./LoaderPicker";
import {
  cancelScheduledPreload,
  preloadLatestCatalogs,
  schedulePreloadCatalog,
} from "./catalogCache";
import { IconClock } from "../resources/icons";
import {
  blockIconSrc,
  typeLabel,
  formatReleaseTime,
} from "../version/instanceVisuals";
import { listRemoteVersions } from "../download/ipc";
import { useSettings } from "../../lib/settings";
import { resolveInstanceRoot } from "../resources/ipc";
import {
  groupVersions,
  VERSION_GROUPS,
  versionGroupOf,
  type VersionGroupId,
} from "../resources/versionGroups";
import type {
  DownloadSource,
  RemoteManifest,
  RemoteVersion,
} from "../download/types";
import "./vanilla.css";
import "./LoaderPicker.css";

/** 折叠分组展示顺序（与 versionGroups 的分组一致）。 */
const GROUP_ORDER: readonly VersionGroupId[] = VERSION_GROUPS.map((g) => g.id);

/** 页面状态机。 */
type PageState = "loading" | "ready" | "error";

/** 远端版本 → Blocks 图标键（与实例卡片的自动映射同口径）。 */
function iconKeyFor(version: RemoteVersion): string {
  if (versionGroupOf(version) === "release") return "grass";
  if (versionGroupOf(version) === "snapshot") return "command_block";
  if (versionGroupOf(version) === "april") return "gold_block";
  return "cobblestone";
}

export function VanillaPage() {
  const { settings } = useSettings();
  const source: DownloadSource = settings.versionListSource;
  const [state, setState] = useState<PageState>("loading");
  const [message, setMessage] = useState("");
  const [manifest, setManifest] = useState<RemoteManifest | null>(null);
  const [manifestSource, setManifestSource] = useState<DownloadSource | null>(
    null
  );
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState("");
  const [expanded, setExpanded] = useState<Record<VersionGroupId, boolean>>({
    release: true,
    snapshot: false,
    old: false,
    april: false,
  });

  // 实例目录（安装目标）。
  const [instanceDir, setInstanceDir] = useState("");

  const [pickerVersion, setPickerVersion] = useState<RemoteVersion | null>(
    null
  );

  // ── 版本清单 ────────────────────────────────────────────────────────────
  const [manifestAttempt, setManifestAttempt] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setState("loading");
    setMessage("");
    setManifestSource(null);
    listRemoteVersions(source)
      .then((result) => {
        if (cancelled) return;
        setManifest(result);
        setManifestSource(source);
        setState("ready");
      })
      .catch((err) => {
        if (cancelled) return;
        setState("error");
        setMessage(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [source, manifestAttempt]);

  useEffect(() => {
    let cancelled = false;
    resolveInstanceRoot()
      .then((root) => {
        if (!cancelled) setInstanceDir(root?.dir ?? "");
      })
      .catch(() => {
        if (!cancelled) setInstanceDir("");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // ── 分组 + 搜索过滤 ────────────────────────────────────────────────────
  const searching = query.trim() !== "";
  const filteredVersions = useMemo(() => {
    if (!manifest) return [];
    const keyword = query.trim().toLowerCase();
    if (!keyword) return manifest.versions;
    return manifest.versions.filter((v) =>
      v.id.toLowerCase().includes(keyword)
    );
  }, [manifest, query]);

  // 搜索首次出现结果时展开分组；展开后仍可由用户自由收起，不伪造只读交互。
  const handleQueryChange = (next: string) => {
    setQuery(next);
    if (next.trim()) {
      setExpanded({ release: true, snapshot: true, old: true, april: true });
    }
  };

  const groups = useMemo(
    () => groupVersions(filteredVersions),
    [filteredVersions]
  );

  const latest = useMemo(() => {
    if (!manifest) return null;
    const release = manifest.versions.find(
      (v) => v.id === manifest.latestRelease
    );
    const snapshot = manifest.versions.find(
      (v) => v.id === manifest.latestSnapshot
    );
    return { release, snapshot };
  }, [manifest]);

  // 页面进入即预热最新正式版与预览版；source 必须与当前 manifest 一致，避免串源。
  useEffect(() => {
    if (state !== "ready" || !manifest || manifestSource !== source) return;
    preloadLatestCatalogs(manifest, source);
  }, [state, manifest, manifestSource, source]);

  useEffect(() => () => cancelScheduledPreload(), []);

  const openPicker = useCallback((version: RemoteVersion) => {
    cancelScheduledPreload();
    setPickerVersion(version);
  }, []);

  // 指针停留或键盘聚焦后才预热，快速掠过版本列表时由防抖取消上一次候选。
  const preloadVersion = useCallback(
    (version: RemoteVersion) => schedulePreloadCatalog(version, source),
    [source]
  );

  // ── 渲染：版本列表 ─────────────────────────────────────────────────────
  const renderVersionList = () => (
    <div
      className={`van-page${state === "loading" ? " van-page--loading" : ""}`}
    >
      <header className="van-page__head">
        <div className="van-page__titles">
          <div className="ni-eyebrow">原版游戏</div>
          <h2 className="van-page__title">线上版本</h2>
        </div>
        <label className="mr-search">
          <IconSearch aria-hidden />
          <input
            type="search"
            value={query}
            onChange={(event) => handleQueryChange(event.target.value)}
            placeholder="按版本号筛选，如 1.21"
            aria-label="按版本号筛选"
          />
        </label>
      </header>

      {state === "loading" ? <LoadingCard label="正在加载" /> : null}

      {state === "error" ? (
        <LoadingCard
          state="error"
          label={failureLabel(message)}
          action={
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setManifestAttempt((value) => value + 1)}
            >
              重试
            </Button>
          }
        />
      ) : null}

      {state === "ready" && manifest ? (
        <div className="van-page__body">
          {/* 搜索时只呈现过滤结果；未搜索时保留最新正式版 / 预览版入口。 */}
          {!searching ? (
            <div className="van-latest">
              {latest?.release ? (
                <LatestCard
                  version={latest.release}
                  label="最新正式版"
                  onPick={openPicker}
                  onPreload={preloadVersion}
                />
              ) : null}
              {latest?.snapshot ? (
                <LatestCard
                  version={latest.snapshot}
                  label="最新预览版"
                  onPick={openPicker}
                  onPreload={preloadVersion}
                />
              ) : null}
            </div>
          ) : null}

          {searching && filteredVersions.length === 0 ? (
            <section className="van-empty" role="status" aria-live="polite">
              <span className="van-empty__icon" aria-hidden="true">
                <IconSearch />
              </span>
              <h3 className="van-empty__title">
                没有匹配“{query.trim()}”的版本
              </h3>
              <p className="van-empty__hint">
                换一个版本号，或清除筛选查看全部版本。
              </p>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setQuery("")}
              >
                清除筛选
              </Button>
            </section>
          ) : null}

          {/* 四个折叠分组。 */}
          {GROUP_ORDER.map((groupId) => {
            const items = groups[groupId];
            if (items.length === 0) return null;
            const groupDef = VERSION_GROUPS.find((g) => g.id === groupId);
            const isOpen = expanded[groupId];
            const bodyId = `van-group-${groupId}`;
            return (
              <section
                key={groupId}
                className="van-group"
                data-open={isOpen}
                data-version-group={groupId}
              >
                <button
                  type="button"
                  className="van-group__head"
                  aria-expanded={isOpen}
                  aria-controls={bodyId}
                  onClick={() =>
                    setExpanded((prev) => ({
                      ...prev,
                      [groupId]: !prev[groupId],
                    }))
                  }
                >
                  <span
                    className="van-group__arrow"
                    aria-hidden
                    data-open={isOpen}
                  >
                    <IconChevronDown />
                  </span>
                  <span className="van-group__title">
                    {groupDef?.label ?? groupId}
                  </span>
                  <span className="van-group__count">
                    {items.length} 个版本
                  </span>
                </button>
                <Collapse
                  id={bodyId}
                  open={isOpen}
                  className="van-group__collapse"
                >
                  <ul className="van-group__list" role="list">
                    {items.map((version) => (
                      <li key={version.id}>
                        <button
                          type="button"
                          className="van-version"
                          data-selected={selectedId === version.id}
                          onPointerEnter={() => preloadVersion(version)}
                          onFocus={() => preloadVersion(version)}
                          onClick={() => {
                            setSelectedId(version.id);
                            openPicker(version);
                          }}
                        >
                          <img
                            className="van-version__icon"
                            src={blockIconSrc(iconKeyFor(version))}
                            alt=""
                            width={28}
                            height={28}
                            draggable={false}
                          />
                          <span className="van-version__id">{version.id}</span>
                          <span className="van-version__meta">
                            <span className="van-version__type">
                              {typeLabel(version.type)}
                            </span>
                            {formatReleaseTime(version.releaseTime) ? (
                              <span className="van-version__date">
                                <IconClock aria-hidden />
                                {formatReleaseTime(version.releaseTime)}
                              </span>
                            ) : null}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </Collapse>
              </section>
            );
          })}
        </div>
      ) : null}
    </div>
  );

  return pickerVersion ? (
    <LoaderPicker
      key={`${pickerVersion.id}:${source}`}
      version={pickerVersion}
      source={source}
      fileSource={settings.fileDownloadSource}
      instanceDir={instanceDir}
      onBack={() => setPickerVersion(null)}
    />
  ) : (
    renderVersionList()
  );
}

/**
 * 最新版本卡（正式版 / 预览版共用）。
 * 图标用 Blocks 资源（正式版=草方块 / 预览版=命令方块），与实例卡片同一映射。
 */
function LatestCard({
  version,
  label,
  onPick,
  onPreload,
}: {
  version: RemoteVersion;
  label: string;
  onPick: (version: RemoteVersion) => void;
  onPreload: (version: RemoteVersion) => void;
}) {
  const iconKey =
    versionGroupOf(version) === "release" ? "grass" : "command_block";
  return (
    <button
      type="button"
      className="van-latest__card"
      onPointerEnter={() => onPreload(version)}
      onFocus={() => onPreload(version)}
      onClick={() => onPick(version)}
    >
      <img
        className="van-latest__icon"
        src={blockIconSrc(iconKey)}
        alt=""
        width={40}
        height={40}
        draggable={false}
      />
      <span className="van-latest__text">
        <span className="van-latest__label">{label}</span>
        <span className="van-latest__id">{version.id}</span>
        {formatReleaseTime(version.releaseTime) ? (
          <span className="van-latest__date">
            <IconClock aria-hidden />
            {formatReleaseTime(version.releaseTime)}
          </span>
        ) : null}
      </span>
    </button>
  );
}
