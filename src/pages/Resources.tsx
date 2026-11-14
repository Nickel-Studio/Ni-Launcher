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
  DetailInstance,
  GroupMemory,
  ProjectDetailAdapter,
} from "../components/community/ProjectDetailPage";
import { useDetailInstance } from "../components/community/searchView";
import { loadResourceInstance } from "../components/resources/ipc";
import "../components/community/community.css";
import {
  MODRINTH_CATEGORIES,
  type ModrinthHit,
  type ModrinthKind,
} from "../components/modrinth";
import {
  openModrinthProject,
  projectVersions,
  saveAsModrinth,
} from "../components/modrinth/ipc";
import {
  curseforgeProjectFiles,
  openCurseforgeProject,
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

/** 当前打开的详情目标（卡片命中 + 所属分类）。 */
interface DetailTarget {
  hit: ModrinthHit;
  kind: CommunityKind;
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
  const searchHost = useRef<HTMLDivElement>(null);
  const scrollPositions = useRef<Array<[HTMLElement, number]>>([]);
  const detailTrigger = useRef<HTMLElement | null>(null);
  const [detail, setDetail] = useState<DetailTarget | null>(null);
  const instance: DetailInstance = useDetailInstance(
    detail ?? page,
    resourceTarget
  );

  useEffect(() => {
    const openMods = () => {
      const target = consumeResourceModsTarget();
      if (!target) return;
      setResourceTarget(target);
      setDetail(null);
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
    const host = searchHost.current;
    const nodes = host
      ? Array.from(host.querySelectorAll<HTMLElement>("*"))
      : [];
    let parent: HTMLElement | null = host;
    while (parent) {
      nodes.push(parent);
      parent = parent.parentElement;
    }
    scrollPositions.current = nodes
      .filter((node) => node.scrollTop > 0)
      .map((node) => [node, node.scrollTop]);
    detailTrigger.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setDetail({ hit, kind });
  }, []);

  const closeDetail = useCallback(() => setDetail(null), []);
  useLayoutEffect(() => {
    if (detail) return;
    for (const [node, top] of scrollPositions.current) node.scrollTop = top;
    const trigger = detailTrigger.current;
    if (trigger?.isConnected) trigger.focus({ preventScroll: true });
    detailTrigger.current = null;
  }, [detail]);

  // 详情页分组展开状态按项目 id 会话级记忆：离开再返回恢复（不落盘）。
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

  const detailAdapter =
    useMemo<ProjectDetailAdapter<ModrinthHit> | null>(() => {
      if (!detail) return null;
      const pack = detail.kind === "modpack";
      // 另存为身份复核：root + versionId + 资源根仍有效才弹框；不强制
      // mcVersion 匹配（默认目录只是起点，用户可另存到任意位置）。
      const revalidateForSaveAs = async () => {
        const current = await loadResourceInstance(resourceTarget ?? undefined);
        if (
          !instance.dir ||
          !current ||
          current.root !== instance.root ||
          current.versionId !== instance.versionId ||
          current.dir !== instance.dir
        ) {
          throw new Error("当前实例已变化，请返回资源列表后重新打开项目。");
        }
        return current;
      };
      const startPack = (
        versionId: string,
        instanceName: string | undefined,
        run: () => Promise<string>
      ) =>
        startManagedDownload(
          {
            versionId,
            instanceName: instanceName ?? detail.hit.title,
            dir: "",
            loader: "",
            loaderVersion: null,
            source: settings.fileDownloadSource,
            kind: "modpack",
          },
          run
        );
      // 普通资源：先复核身份，再交给后端另存为对话框；null=取消不建任务。
      const startSaveAs = (
        versionId: string,
        run: (current: DetailInstance) => Promise<string | null>
      ) =>
        revalidateForSaveAs().then((current) =>
          startManagedDownload(
            {
              versionId,
              instanceName: current.label || current.versionId,
              dir: current.dir,
              loader: "",
              loaderVersion: null,
              source: settings.fileDownloadSource,
              kind: "resource",
            },
            () => run(current)
          )
        );
      if (detail.hit.source === "curseforge") {
        const projectId = detail.hit.projectId ?? "";
        return {
          openProject: () =>
            openCurseforgeProject(detail.kind, detail.hit.slug),
          fetchVersions: (_hit, query) =>
            curseforgeProjectFiles({ kind: detail.kind, projectId, ...query }),
          install: (_hit, version, instanceName) =>
            pack
              ? startPack(version.id, instanceName, () =>
                  installModpack({
                    platform: "curseforge",
                    projectId,
                    versionId: version.id,
                    instanceName,
                  })
                )
              : startSaveAs(version.id, (current) =>
                  saveAsCurseforge({
                    kind: detail.kind,
                    projectId,
                    fileId: version.id,
                    root: current.root,
                    instanceVersionId: current.versionId,
                    dir: current.dir,
                    source: settings.fileDownloadSource,
                  })
                ),
        };
      }
      return {
        openProject: () => openModrinthProject(detail.hit.slug),
        fetchVersions: async (_hit, query) => {
          const versions = await projectVersions({
            kind: detail.kind,
            slugOrId: detail.hit.slug,
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
        install: (_hit, version, instanceName) =>
          pack
            ? startPack(version.id, instanceName, () =>
                installModpack({
                  platform: "modrinth",
                  projectId: detail.hit.slug,
                  versionId: version.id,
                  instanceName,
                })
              )
            : startSaveAs(version.id, (current) =>
                saveAsModrinth({
                  kind: detail.kind,
                  slugOrId: detail.hit.slug,
                  versionId: version.id,
                  root: current.root,
                  instanceVersionId: current.versionId,
                  dir: current.dir,
                  source: settings.fileDownloadSource,
                })
              ),
      };
    }, [detail, instance, resourceTarget, settings.fileDownloadSource]);

  return (
    <>
      {detail && detailAdapter ? (
        <ProjectDetailPage
          key={detail.hit.source + ":" + detail.hit.slug}
          kind={detail.kind}
          hit={detail.hit}
          adapter={detailAdapter}
          instance={instance}
          onBack={closeDetail}
          groupMemory={groupMemory}
        />
      ) : null}
      <div
        ref={searchHost}
        className="resources-browser"
        hidden={Boolean(detail)}
      >
        <PagerLayout
          className="nw-pager--fill"
          items={navItems}
          active={page}
          onSelect={(id) => setPage(id)}
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
              onOpenDetail={openDetail}
            />
          )}
        </PagerLayout>
      </div>
    </>
  );
}
