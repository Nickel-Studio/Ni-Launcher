import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { PagerLayout, type PagerItem } from "../components/ui";
import { VanillaPage } from "../components/vanilla";
import "../components/vanilla/vanilla.css";
import {
  CommunitySearchPage,
  ProjectDetailPage,
} from "../components/community";
import { CommunityKindIcon } from "../components/community/CommunityKindIcon";
import type {
  DependencyContext,
  DetailInstance,
  GroupMemory,
  ProjectDetailAdapter,
} from "../components/community/ProjectDetailPage";
import {
  captureResourceScroll,
  dependencyKind,
  restoreResourceScroll,
  type ResourceScrollSnapshot,
} from "../components/community/dependencies";
import { useDetailInstance } from "../components/community/searchView";
import { loadResourceInstance } from "../components/resources/ipc";
import "../components/community/community.css";
import {
  MODRINTH_CATEGORIES,
  type ModrinthHit,
  type ModrinthKind,
} from "../components/modrinth";
import type {
  ModrinthVersion,
  ResolvedDependency,
} from "../components/modrinth/types";
import {
  openModrinthProject,
  projectVersions,
  resolveModrinthDependency,
  saveAsModrinth,
} from "../components/modrinth/ipc";
import {
  curseforgeProjectFiles,
  openCurseforgeProject,
  resolveCurseforgeDependency,
  saveAsCurseforge,
} from "../components/curseforge/ipc";
import "../components/modrinth/modrinth.css";
import "./Resources.css";
import { useSettings } from "../lib/settings";
import { startManagedDownload } from "../components/download/store";
import { installModpack } from "../components/community/packIpc";
import { consumeResourceModsTarget, RESOURCE_MODS_EVENT } from "../lib/nav";
import type { VersionFocus } from "../lib/instances";

/**
 * 「资源」tab = 下载中心：
 * 侧边栏两组——「原版游戏」（线上版本下载页：最新版本 + 折叠分组 + 加载器步骤）
 * 与「社区资源」（Modrinth / CurseForge 双引擎检索）。
 * 点社区资源结果卡 → detail 状态提升到本页：独立详情页全宽替换整个右内容区，
 * 侧边栏暂时消失（用户裁定：展开版本不可用，详情必须是独立页）；返回即恢复。
 * 已安装内容的查看入口在版本设置页，这里不再出现本地扫描分组。
 */

/** 社区分类 id（与 Modrinth / CurseForge 后端的 kind 参数一致）。 */
type CommunityKind = ModrinthKind;

/** Each detail frame retains its own filters, disclosures and DOM while hidden. */
interface DetailTarget {
  id: number;
  hit: ModrinthHit;
  kind: CommunityKind;
  context?: DependencyContext;
}
interface SaveContext {
  instance: DetailInstance;
  resourceTarget: VersionFocus | null;
  source: import("../lib/settings").SourcePolicy;
}

function createDetailAdapter(
  target: DetailTarget,
  current: () => SaveContext
): ProjectDetailAdapter<ModrinthHit> {
  const { kind } = target;
  const pack = kind === "modpack";
  const revalidateForSaveAs = async (expected: SaveContext) => {
    const resolved = await loadResourceInstance(
      expected.resourceTarget ?? undefined
    );
    const instance = expected.instance;
    if (
      !instance.dir ||
      !resolved ||
      resolved.root !== instance.root ||
      resolved.versionId !== instance.versionId ||
      resolved.dir !== instance.dir
    ) {
      throw new Error("当前实例已变化，请返回资源列表后重新打开项目。");
    }
    return resolved;
  };
  return {
    openProject: (hit) =>
      hit.source === "curseforge"
        ? openCurseforgeProject(kind, hit.slug)
        : openModrinthProject(hit.slug),
    fetchVersions: async (hit, query) => {
      if (hit.source === "curseforge") {
        return curseforgeProjectFiles({
          kind,
          projectId: hit.projectId ?? "",
          ...query,
        });
      }
      const versions = await projectVersions({
        kind,
        slugOrId: hit.projectId || hit.slug,
        gameVersion: query.gameVersion,
        loader: query.loader,
      });
      return {
        versions,
        offset: 0,
        nextOffset: versions.length,
        hasMore: false,
        total: versions.length,
      };
    },
    resolveDependency:
      target.hit.source === "curseforge"
        ? resolveCurseforgeDependency
        : resolveModrinthDependency,
    install: async (hit, version, instanceName) => {
      const expected = current();
      if (pack) {
        return startManagedDownload(
          {
            versionId: version.id,
            instanceName: instanceName ?? hit.title,
            dir: "",
            loader: "",
            loaderVersion: null,
            source: expected.source,
            kind: "modpack",
          },
          () =>
            installModpack({
              platform: hit.source,
              projectId: hit.projectId || hit.slug,
              versionId: version.id,
              instanceName,
            })
        );
      }
      const resolved = await revalidateForSaveAs(expected);
      return startManagedDownload(
        {
          versionId: version.id,
          instanceName: resolved.label || resolved.versionId,
          dir: resolved.dir,
          loader: "",
          loaderVersion: null,
          source: expected.source,
          kind: "resource",
        },
        () =>
          hit.source === "curseforge"
            ? saveAsCurseforge({
                kind,
                projectId: hit.projectId ?? "",
                fileId: version.id,
                root: resolved.root,
                instanceVersionId: resolved.versionId,
                dir: resolved.dir,
                source: expected.source,
              })
            : saveAsModrinth({
                kind,
                slugOrId: hit.projectId || hit.slug,
                versionId: version.id,
                root: resolved.root,
                instanceVersionId: resolved.versionId,
                dir: resolved.dir,
                source: expected.source,
              })
      );
    },
  };
}

/** 原版游戏页的侧边栏占位 id（对应 VanillaPage）。 */
const VANILLA_ID = "vanilla";

function IconGamepad() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      aria-hidden
    >
      <rect x="1.75" y="4.5" width="12.5" height="7" rx="3.5" />
      <path
        d="M5 6.8v2.4M3.8 8h2.4M10.6 7.2h.01M12.2 8.9h.01"
        strokeLinecap="round"
      />
    </svg>
  );
}

export default function Resources() {
  const { settings } = useSettings();
  const [page, setPage] = useState<string>(VANILLA_ID);
  const [resourceTarget, setResourceTarget] = useState<VersionFocus | null>(
    null
  );
  const [details, setDetails] = useState<DetailTarget[]>([]);
  const detail = details[details.length - 1];
  const nextDetailId = useRef(0);
  const searchHost = useRef<HTMLDivElement>(null);
  const detailHosts = useRef(new Map<number, HTMLDivElement>());
  const returnPositions = useRef(
    new Map<number | "search", ResourceScrollSnapshot>()
  );
  const pendingScroll = useRef<ResourceScrollSnapshot | "top" | null>(null);
  const instance = useDetailInstance(page, resourceTarget);
  const saveContext = useRef<SaveContext>({
    instance,
    resourceTarget,
    source: settings.fileDownloadSource,
  });
  saveContext.current = {
    instance,
    resourceTarget,
    source: settings.fileDownloadSource,
  };
  const adapterCache = useRef(
    new Map<number, ProjectDetailAdapter<ModrinthHit>>()
  );
  const adapters = useMemo(() => {
    const ids = new Set(details.map((frame) => frame.id));
    for (const id of adapterCache.current.keys()) {
      if (!ids.has(id)) adapterCache.current.delete(id);
    }
    return details.map((frame) => {
      let adapter = adapterCache.current.get(frame.id);
      if (!adapter) {
        adapter = createDetailAdapter(frame, () => saveContext.current);
        adapterCache.current.set(frame.id, adapter);
      }
      return adapter;
    });
  }, [details]);

  useEffect(() => {
    const openMods = () => {
      const target = consumeResourceModsTarget();
      if (!target) return;
      setResourceTarget(target);
      setDetails([]);
      returnPositions.current.clear();
      pendingScroll.current = "top";
      setPage("mod");
    };
    window.addEventListener(RESOURCE_MODS_EVENT, openMods);
    openMods();
    return () => window.removeEventListener(RESOURCE_MODS_EVENT, openMods);
  }, []);

  const navItems = useMemo<PagerItem[]>(
    () => [
      {
        id: VANILLA_ID,
        label: "原版游戏",
        icon: <IconGamepad />,
        group: "原版游戏",
      },
      ...MODRINTH_CATEGORIES.map((item) => ({
        id: item.id,
        label: item.label,
        icon: <CommunityKindIcon kind={item.id} />,
        group: "社区资源",
      })),
    ],
    []
  );
  const activeItem = navItems.find((item) => item.id === page);

  const openDetail = useCallback((hit: ModrinthHit, kind: CommunityKind) => {
    returnPositions.current.set(
      "search",
      captureResourceScroll(searchHost.current)
    );
    pendingScroll.current = "top";
    setDetails([{ id: ++nextDetailId.current, hit, kind }]);
  }, []);
  const openDependency = useCallback(
    (
      parent: DetailTarget,
      resolved: ResolvedDependency,
      version: ModrinthVersion
    ) => {
      returnPositions.current.set(
        parent.id,
        captureResourceScroll(detailHosts.current.get(parent.id) ?? null)
      );
      pendingScroll.current = "top";
      setDetails((frames) => [
        ...frames,
        {
          id: ++nextDetailId.current,
          hit: resolved.project,
          kind: dependencyKind(resolved.project),
          context: {
            parentTitle: parent.hit.title,
            parentVersion: version,
            requiredVersion: resolved.version,
          },
        },
      ]);
    },
    []
  );
  const closeDetail = useCallback(() => {
    const parent = details[details.length - 2];
    pendingScroll.current =
      returnPositions.current.get(parent?.id ?? "search") ?? null;
    if (detail) returnPositions.current.delete(detail.id);
    setDetails((frames) => frames.slice(0, -1));
  }, [details, detail]);

  useLayoutEffect(() => {
    const pending = pendingScroll.current;
    if (!pending) return;
    const restore = () => {
      if (pending !== "top") {
        restoreResourceScroll(pending);
        return;
      }
      let node: HTMLElement | null = detail
        ? detailHosts.current.get(detail.id) ?? null
        : searchHost.current;
      while (node) {
        node.scrollTop = 0;
        node = node.parentElement;
      }
    };
    restore();
    const frame = window.requestAnimationFrame(restore);
    pendingScroll.current = null;
    return () => window.cancelAnimationFrame(frame);
  }, [detail]);

  const groupMemoryRef = useRef(new Map<string, ReadonlySet<string>>());
  const groupMemory = useMemo<GroupMemory>(
    () => ({
      read: (id) => groupMemoryRef.current.get(id),
      write: (id, open) => {
        groupMemoryRef.current.set(id, open);
      },
    }),
    []
  );

  return (
    <>
      {details.length ? (
        <div className="resources-details">
          {details.map((frame, index) => (
            <div
              key={frame.id}
              ref={(node) => {
                if (node) detailHosts.current.set(frame.id, node);
                else detailHosts.current.delete(frame.id);
              }}
              className="resources-detail"
              hidden={frame !== detail}
              inert={frame !== detail}
              aria-hidden={frame !== detail}
              data-detail-id={frame.id}
            >
              <ProjectDetailPage
                kind={frame.kind}
                hit={frame.hit}
                adapter={adapters[index]}
                instance={instance}
                onBack={closeDetail}
                backLabel={index > 0 ? "返回上级资源" : "返回检索"}
                groupMemory={groupMemory}
                dependencyContext={frame.context}
                onOpenDependency={(resolved, version) =>
                  openDependency(frame, resolved, version)
                }
              />
            </div>
          ))}
        </div>
      ) : null}
      <div
        ref={searchHost}
        className="resources-browser"
        hidden={Boolean(detail)}
        inert={Boolean(detail)}
      >
        <PagerLayout
          className="ni-pager--fill"
          items={navItems}
          active={page}
          onSelect={setPage}
          navLabel="资源分类"
          navTitle="下载中心"
        >
          {page === VANILLA_ID ? (
            <VanillaPage />
          ) : (
            <CommunitySearchPage
              kind={page as CommunityKind}
              label={activeItem?.label ?? page}
              target={resourceTarget}
              active={!detail}
              onOpenDetail={openDetail}
            />
          )}
        </PagerLayout>
      </div>
    </>
  );
}
