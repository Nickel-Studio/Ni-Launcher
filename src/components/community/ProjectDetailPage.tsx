/** 另存对话框取消不建任务；整合包先确认名称。全量历史仅在本地筛选。 */
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Button, Collapse, LoadingCard, Modal, Tooltip } from "../ui";
import { IconChevronDown, IconDownload, IconGlobe } from "../ui/icons";
import { IconAlert } from "../resources/icons";
import { formatBytes } from "../resources/format";
import type { ResourceInstance } from "../resources/types";
import { formatDownloads, formatIsoDate } from "../modrinth/parse";
import type {
  ModrinthHit,
  ModrinthKind,
  ModrinthVersion,
  ProjectVersionPage,
} from "../modrinth/types";
import { CommunityKindIcon } from "./CommunityKindIcon";
import { RELEASE_TYPES } from "./releaseTypes";
import {
  deriveFileDisplayName,
  fileStem,
  formatRelativeTime,
} from "./versionDisplay";
import {
  ALL_FILTER,
  buildVersionGroups,
  defaultFilterFor,
  deriveVersionPills,
  loaderLabel,
  type VersionGroup,
} from "./versionFilters";
import "./projectDetail.css";

export interface VersionQuery {
  offset: number;
  gameVersion: string;
  loader: string;
}
export interface ProjectDetailAdapter<K> {
  openProject: (hit: K) => Promise<unknown>;
  fetchVersions: (hit: K, query: VersionQuery) => Promise<ProjectVersionPage>;
  /** 普通资源 = 另存为（taskId 或 null=取消）；整合包 = 命名后安装。 */
  install: (
    hit: K,
    version: ModrinthVersion,
    instanceName?: string
  ) => Promise<string | null>;
}
/** 与搜索和安装复核共用真实 MC 版本字段，不另用含糊的 version 别名。 */
export type DetailInstance = ResourceInstance;
/** 会话级展开状态记忆（Resources 层按项目 id 持有，离开再返回恢复）。 */
export interface GroupMemory {
  read: (projectId: string) => ReadonlySet<string> | undefined;
  write: (projectId: string, open: ReadonlySet<string>) => void;
}
interface Props<K> {
  kind: ModrinthKind;
  hit: K;
  adapter: ProjectDetailAdapter<K>;
  instance: DetailInstance;
  onBack: () => void;
  groupMemory?: GroupMemory;
}
function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
function primaryFile(version: ModrinthVersion) {
  return version.files.find((entry) => entry.primary) ?? version.files[0];
}

/**
 * 列表项懒挂载：未进入视口前只占一行高度，进入后才渲染真实行。
 * 分组收起时 Collapse 裁切使占位不相交，不会提前挂载。
 */
function LazyRow({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLLIElement>(null);
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    if (mounted) return;
    const node = ref.current;
    if (!node || typeof IntersectionObserver === "undefined") {
      setMounted(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setMounted(true);
          observer.disconnect();
        }
      },
      { rootMargin: "240px 0px" }
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [mounted]);
  return (
    <li ref={ref} className="pd-row-slot" data-mounted={mounted || undefined}>
      {mounted ? children : null}
    </li>
  );
}

export function ProjectDetailPage<K extends ModrinthHit>({
  kind,
  hit,
  adapter,
  instance,
  onBack,
  groupMemory,
}: Props<K>) {
  const pack = kind === "modpack";
  const projectId = `${hit.source}:${hit.slug}`;
  const [versions, setVersions] = useState<ModrinthVersion[]>([]);
  const [loading, setLoading] = useState(true);
  const [listError, setListError] = useState("");
  const [actionError, setActionError] = useState("");
  const [installingId, setInstallingId] = useState("");
  const [startedId, setStartedId] = useState("");
  const [pendingVersion, setPendingVersion] = useState<ModrinthVersion | null>(
    null
  );
  const [instanceName, setInstanceName] = useState(hit.title);
  const [nameError, setNameError] = useState("");
  const [nonce, setNonce] = useState(0);
  const [filter, setFilter] = useState(ALL_FILTER);
  const [readyToken, setReadyToken] = useState(0);
  const nameId = useId();
  const formId = useId();
  const groupsId = useId();
  const [openGroups, setOpenGroups] = useState<Set<string>>(() => new Set());
  const nameInput = useRef<HTMLInputElement>(null);
  const backButton = useRef<HTMLButtonElement | null>(null);
  const installTrigger = useRef<HTMLButtonElement | null>(null);
  const generation = useRef(0);
  const installBusy = useRef(false);
  const alive = useRef(true);
  const userFilter = useRef(false);
  const initializedToken = useRef(0);
  const lastInstanceKey = useRef("");
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    backButton.current?.focus();
  }, []);
  useEffect(() => {
    const request = ++generation.current;
    setLoading(true);
    setListError("");
    setVersions([]);
    setFilter(ALL_FILTER);
    userFilter.current = false;
    // 全量历史一次拉取（CurseForge 分页在内部循环到尽），页内不再提供
    // 「加载往期版本」按钮；筛选与加载器分组只在本地显示，不截断历史。
    void (async () => {
      try {
        const merged = new Map<string, ModrinthVersion>();
        let offset = 0;
        for (;;) {
          const result = await adapter.fetchVersions(hit, {
            offset,
            gameVersion: "",
            loader: "",
          });
          if (request !== generation.current) return;
          for (const item of result.versions) merged.set(item.id, item);
          if (!result.hasMore || result.nextOffset <= offset) break;
          offset = result.nextOffset;
        }
        if (request !== generation.current) return;
        setVersions([...merged.values()]);
        setReadyToken(request);
      } catch (error: unknown) {
        if (request === generation.current) setListError(errorText(error));
      } finally {
        if (request === generation.current) setLoading(false);
      }
    })();
    return () => {
      generation.current++;
    };
  }, [hit, adapter, nonce]);
  useLayoutEffect(() => {
    if (pendingVersion) {
      nameInput.current?.focus();
      nameInput.current?.select();
    } else if (!installingId) {
      // React 提交后按钮已解除 disabled；不能用 rAF 猜测提交时序。
      installTrigger.current?.focus();
    }
  }, [pendingVersion, installingId]);

  const filterState = useMemo(
    () =>
      deriveVersionPills(versions.flatMap((version) => version.gameVersions)),
    [versions]
  );
  const groups = useMemo(
    () =>
      buildVersionGroups({
        versions,
        kind,
        projectLoaders: hit.loaders,
        filter,
        groupedByDrop: filterState.groupedByDrop,
        foldOld: filterState.foldOld,
        instanceVersion: instance.mcVersion,
        instanceLoader: instance.loader,
      }),
    [
      versions,
      kind,
      hit.loaders,
      filter,
      filterState,
      instance.mcVersion,
      instance.loader,
    ]
  );

  // 每次完整加载后就绪一次：默认筛选跟随实例 mcVersion；展开状态优先恢复
  // 会话记忆，否则「所选版本」卡展开、只有一张卡时强制展开。实例信息晚到
  // （异步复核）时补一次默认值，不覆盖用户手动选择的筛选。
  const instanceKey = `${instance.mcVersion}|${instance.loader}`;
  useEffect(() => {
    if (loading || readyToken === 0) return;
    const firstInit = initializedToken.current !== readyToken;
    const instanceChanged = lastInstanceKey.current !== instanceKey;
    if (!firstInit && !instanceChanged) return;
    initializedToken.current = readyToken;
    lastInstanceKey.current = instanceKey;
    const trimmedVersion = instance.mcVersion.trim();
    const supported =
      trimmedVersion !== "" &&
      versions.some((version) => version.gameVersions.includes(trimmedVersion));
    const nextFilter = defaultFilterFor(
      filterState,
      instance.mcVersion,
      supported
    );
    const applyFilter = firstInit || !userFilter.current;
    if (applyFilter) setFilter(nextFilter);
    const initialGroups = buildVersionGroups({
      versions,
      kind,
      projectLoaders: hit.loaders,
      filter: applyFilter ? nextFilter : filter,
      groupedByDrop: filterState.groupedByDrop,
      foldOld: filterState.foldOld,
      instanceVersion: instance.mcVersion,
      instanceLoader: instance.loader,
    });
    if (firstInit) {
      const remembered = groupMemory?.read(projectId);
      const open = remembered
        ? new Set(
            [...remembered].filter((key) =>
              initialGroups.some((group) => group.key === key)
            )
          )
        : new Set(
            initialGroups
              .filter((group) => group.selected)
              .map((group) => group.key)
          );
      if (initialGroups.length === 1) open.add(initialGroups[0].key);
      setOpenGroups(open);
      groupMemory?.write(projectId, open);
    } else {
      const next = new Set(openGroups);
      for (const group of initialGroups) {
        if (group.selected) next.add(group.key);
      }
      setOpenGroups(next);
      groupMemory?.write(projectId, next);
    }
    // 依赖刻意只含就绪令牌与实例身份：实例信息晚到时补默认筛选，不覆盖用户手动选择。
  }, [loading, readyToken, instanceKey]);

  // 同组只要出现重名，就整组回退，避免同一列表混用两种命名规则。
  const displayNames = useMemo(() => {
    const byGroup = new Map<string, Map<string, string>>();
    for (const group of groups) {
      const derived = group.items.map((version) => {
        const filename =
          primaryFile(version)?.filename ?? version.versionNumber;
        return {
          id: version.id,
          name: deriveFileDisplayName({
            filename,
            versionNumber: version.versionNumber,
            gameVersions: version.gameVersions,
          }),
          stem: fileStem(filename),
        };
      });
      const counts = new Map<string, number>();
      for (const item of derived) {
        counts.set(item.name, (counts.get(item.name) ?? 0) + 1);
      }
      const hasDuplicates = [...counts.values()].some((count) => count > 1);
      byGroup.set(
        group.key,
        new Map(
          derived.map((item) => [
            item.id,
            hasDuplicates ? item.stem : item.name,
          ])
        )
      );
    }
    return byGroup;
  }, [groups]);

  function toggleGroup(key: string) {
    const next = new Set(openGroups);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setOpenGroups(next);
    groupMemory?.write(projectId, next);
  }
  function selectFilter(value: string) {
    userFilter.current = true;
    setFilter(value);
  }

  async function install(version: ModrinthVersion, name?: string) {
    if (installBusy.current || (!pack && !instance.dir)) return;
    installBusy.current = true;
    setActionError("");
    setNameError("");
    setInstallingId(version.id);
    // 等待队列也需要可取消，确认后先让出弹窗，再由下载浮窗显示任务。
    if (pack) setPendingVersion(null);
    try {
      const taskId = await adapter.install(hit, version, name);
      if (alive.current && taskId) setStartedId(version.id);
    } catch (error) {
      if (alive.current) {
        if (pack) {
          setNameError(errorText(error));
          setPendingVersion(version);
        } else setActionError(errorText(error));
      }
    } finally {
      installBusy.current = false;
      if (alive.current) setInstallingId("");
    }
  }
  function requestInstall(
    version: ModrinthVersion,
    trigger: HTMLButtonElement
  ) {
    if (pack) {
      installTrigger.current = trigger;
      setInstanceName(hit.title);
      setNameError("");
      setPendingVersion(version);
    } else void install(version);
  }
  function closeNaming() {
    if (!installBusy.current) setPendingVersion(null);
  }
  async function openProject() {
    setActionError("");
    try {
      await adapter.openProject(hit);
    } catch (error) {
      if (alive.current) setActionError(errorText(error));
    }
  }
  const date = formatIsoDate(hit.dateModified);
  return (
    <div className="pd-page">
      <div className="pd-page__topbar">
        <Button
          ref={backButton}
          variant="ghost"
          size="sm"
          iconLeft={<IconChevronDown className="pd-back__icon" />}
          onClick={onBack}
        >
          返回检索
        </Button>
      </div>
      <section className="pd-card" aria-label="项目信息">
        <div className="pd-card__head">
          <span className="mr-card__icon" aria-hidden="true">
            {hit.iconUrl ? (
              <img src={hit.iconUrl} alt="" />
            ) : (
              <CommunityKindIcon kind={kind} />
            )}
          </span>
          <div className="pd-card__heading">
            <h3 className="pd-card__title">{hit.title}</h3>
            <div className="pd-card__meta">
              {hit.author ? <span>{hit.author}</span> : null}
              <span>{formatDownloads(hit.downloads)} 下载</span>
              <span className="mr-card__source">
                {hit.source === "curseforge" ? "CurseForge" : "Modrinth"}
              </span>
              {date ? <span>更新于 {date}</span> : null}
            </div>
          </div>
          <Button
            variant="secondary"
            size="sm"
            iconLeft={<IconGlobe />}
            onClick={() => void openProject()}
          >
            打开项目页
          </Button>
        </div>
        {hit.description ? (
          <p className="pd-card__desc">{hit.description}</p>
        ) : null}
        {actionError ? (
          <p className="pd-error" role="alert">
            <IconAlert aria-hidden />
            {actionError}
          </p>
        ) : null}
      </section>
      <section className="pd-downloads" aria-label="版本下载">
        {filterState.pills.length >= 2 ? (
          <div className="pd-pills" role="group" aria-label="版本筛选">
            {[ALL_FILTER, ...filterState.pills].map((pill) => (
              <button
                key={pill}
                type="button"
                className="pd-pill"
                aria-pressed={filter === pill}
                onClick={() => selectFilter(pill)}
              >
                {pill}
              </button>
            ))}
          </div>
        ) : null}
        {!pack && !instance.dir ? (
          <p className="pd-card__empty">
            到「实例与版本」选择目标实例后，可另存为资源文件。
          </p>
        ) : null}
        {loading ? <LoadingCard size="inline" label="正在加载" /> : null}
        {listError ? (
          <div className="pd-error" role="alert">
            <IconAlert aria-hidden />
            <p>{listError}</p>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setNonce((n) => n + 1)}
            >
              重试
            </Button>
          </div>
        ) : null}
        {!loading && !listError && groups.length === 0 ? (
          <p className="pd-card__empty">
            {filter !== ALL_FILTER
              ? "当前筛选下没有版本，请切换版本筛选。"
              : "该项目没有可安装的版本。"}
          </p>
        ) : null}
        {groups.map((group: VersionGroup, groupIndex) => {
          const open = openGroups.has(group.key);
          const panelId = `${groupsId}-${groupIndex}`;
          const titleId = `${panelId}-title`;
          const showLoaderChip = kind === "mod" && !group.hasLoaderPrefix;
          const names = displayNames.get(group.key);
          return (
            <section
              className="pd-group"
              key={group.key}
              data-open={open}
              data-selected={group.selected || undefined}
              aria-labelledby={titleId}
            >
              <button
                type="button"
                className="pd-group__toggle"
                aria-expanded={open}
                aria-controls={panelId}
                onClick={() => toggleGroup(group.key)}
              >
                <span id={titleId} className="pd-group__title">
                  {group.title}
                </span>
                <span className="pd-group__count">
                  {group.items.length} 个版本
                </span>
                <IconChevronDown
                  className="pd-group__chevron"
                  aria-hidden="true"
                />
              </button>
              <Collapse id={panelId} open={open} className="pd-group__collapse">
                {group.other ? (
                  <p className="pd-group__hint">
                    刚发布的游戏版本可能暂未收录，稍后再来查看。
                  </p>
                ) : null}
                <ul className="pd-list">
                  {group.items.map((version) => {
                    const file = primaryFile(version);
                    const type = RELEASE_TYPES[version.releaseType];
                    const displayName =
                      names?.get(version.id) ??
                      fileStem(file?.filename ?? version.versionNumber);
                    const relativeTime = formatRelativeTime(
                      version.datePublished
                    );
                    const label =
                      (pack ? "安装 " : "另存为 ") +
                      displayName +
                      " " +
                      version.versionNumber;
                    return (
                      <LazyRow key={version.id}>
                        <Tooltip
                          label={[
                            type.label,
                            version.name || version.versionNumber,
                            file?.filename,
                            formatIsoDate(version.datePublished),
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                        >
                          <button
                            type="button"
                            className="pd-row"
                            disabled={
                              (!pack && !instance.dir) || Boolean(installingId)
                            }
                            onClick={(event) =>
                              requestInstall(version, event.currentTarget)
                            }
                            aria-label={label}
                          >
                            <img
                              className="pd-row__release"
                              src={type.image}
                              alt=""
                            />
                            <span className="pd-row__main">
                              <span className="pd-row__name">
                                {displayName}
                              </span>
                              <span className="pd-row__number">
                                {version.versionNumber}
                              </span>
                            </span>
                            <span className="pd-row__meta">
                              {showLoaderChip && version.loaders.length ? (
                                <span className="pd-row__loader">
                                  {version.loaders
                                    .map((loader) => loaderLabel(loader))
                                    .join(" / ")}
                                </span>
                              ) : null}
                              {file ? (
                                <span className="pd-row__size">
                                  {formatBytes(file.sizeBytes)}
                                </span>
                              ) : null}
                              {relativeTime ? (
                                <span className="pd-row__time">
                                  {relativeTime}
                                </span>
                              ) : null}
                            </span>
                            <span className="pd-row__status">
                              {installingId === version.id ? (
                                "正在创建任务"
                              ) : startedId === version.id ? (
                                "已开始下载"
                              ) : (
                                <IconDownload />
                              )}
                            </span>
                          </button>
                        </Tooltip>
                      </LazyRow>
                    );
                  })}
                </ul>
              </Collapse>
            </section>
          );
        })}
      </section>
      <Modal
        open={pendingVersion !== null}
        onClose={closeNaming}
        title="输入实例名称"
        className="pd-name-modal"
        footer={
          <>
            <Button
              variant="secondary"
              onClick={closeNaming}
              disabled={Boolean(installingId)}
            >
              取消
            </Button>
            <Button
              type="submit"
              form={formId}
              disabled={Boolean(installingId)}
            >
              {installingId ? "正在创建任务" : "开始安装"}
            </Button>
          </>
        }
      >
        <form
          id={formId}
          className="pd-name-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (!instanceName.trim()) {
              setNameError("请输入实例名称。");
              nameInput.current?.focus();
              return;
            }
            if (pendingVersion) void install(pendingVersion, instanceName);
          }}
        >
          <label htmlFor={nameId}>实例名称</label>
          <input
            ref={nameInput}
            id={nameId}
            className="pd-name-input"
            value={instanceName}
            autoFocus
            autoComplete="off"
            disabled={Boolean(installingId)}
            aria-invalid={Boolean(nameError)}
            aria-describedby={nameError ? nameId + "-error" : undefined}
            onChange={(event) => {
              setInstanceName(event.target.value);
              setNameError("");
            }}
          />
          {nameError ? (
            <p id={nameId + "-error"} className="pd-error" role="alert">
              {nameError}
            </p>
          ) : null}
        </form>
      </Modal>
    </div>
  );
}
