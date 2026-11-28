/**
 * 社区资源检索页：单页头、工具栏、结果列表与底部分页，不显示结果计数。
 * 来源=全部时并行查询两源；切换分类、关键词或筛选回到第 1 页。
 * 返回第一页沿用正常翻页流程，回到顶部只滚动当前活动容器。
 * 实例的真实 MC 版本与加载器作为默认筛选；识别失败静默回退全部。
 * 点结果卡通过 onOpenDetail 进入由 Resources 保留的独立详情页。
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Select } from "../ui";
import { useSettings } from "../../lib/settings";
import { IconSearch } from "../modrinth/icons";
import { ModrinthSearchPage } from "../modrinth/ModrinthSearchPage";
import { CurseforgeSearchPage } from "../curseforge/CurseforgeSearchPage";
import { listRemoteVersions } from "../download/ipc";
import { useDetailInstance } from "./searchView";
import type { VersionFocus } from "../../lib/instances";
import { McVersionCombobox } from "./McVersionCombobox";
import { LoaderSelect } from "./LoaderSelect";
import { CommunityPagination } from "./CommunityPagination";
import { useCombinedSearch } from "./useCombinedSearch";
import { useModrinthSearch } from "../modrinth/useModrinthSearch";
import { useCurseforgeSearch } from "../curseforge/useCurseforgeSearch";
import type {
  ModrinthFilter,
  ModrinthHit,
  ModrinthKind,
} from "../modrinth/types";

/**
 * 检索来源（页面级状态，不持久化——每次进页默认「全部来源」）。
 * "all" = 两源合并检索；CurseForge 未配置 key 时静默退化为仅 Modrinth。
 */
export type CommunitySource = "all" | "modrinth" | "curseforge";

/** 来源下拉选项（Q1 裁定：下拉替代胶囊 tab；R4 裁定：新增「全部来源」并设为默认）。 */
const SOURCE_OPTIONS: ReadonlyArray<{ value: CommunitySource; label: string }> =
  [
    { value: "all", label: "全部来源" },
    { value: "modrinth", label: "Modrinth" },
    { value: "curseforge", label: "CurseForge" },
  ];

/**
 * 工具栏的加载器选项（value 与后端检索参数同源）。
 * 两个引擎的可过滤白名单一致（forge / neoforge / fabric，见后端
 * modrinth.rs / curseforge.rs）；不含白名单之外的选项，避免「选了但后端
 * 不过滤」的假交互。
 */
const LOADER_OPTIONS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "forge", label: "Forge" },
  { value: "neoforge", label: "NeoForge" },
  { value: "fabric", label: "Fabric" },
];

/** 静默识别结果 → 可检索的加载器默认值；vanilla / optifine / 未知落「全部加载器」。 */
const RETRIEVABLE_LOADERS: ReadonlySet<string> = new Set([
  "forge",
  "neoforge",
  "fabric",
]);

interface Props {
  kind: ModrinthKind;
  label: string;
  target?: VersionFocus | null;
  active?: boolean;
  /** 打开独立详情页；隐藏检索页时保留筛选和结果。 */
  onOpenDetail: (hit: ModrinthHit, kind: ModrinthKind) => void;
}

export function CommunitySearchPage({
  kind,
  label,
  target,
  active = true,
  onOpenDetail,
}: Props) {
  const { settings } = useSettings();
  const [source, setSource] = useState<CommunitySource>("all");
  const [query, setQuery] = useState("");
  const [mcVersion, setMcVersion] = useState("");
  const [loader, setLoader] = useState("");
  /** MC 版本下拉的数据源（`list_remote_versions` 的 versions[].id）。 */
  const [versionIds, setVersionIds] = useState<readonly string[]>([]);
  /** 当前页码（1 起计；所有来源共用，条件变化即重置）。 */
  const [page, setPage] = useState(1);

  // 与详情共用实例快照；首页目标变化、安装完成及识别失败都会清除旧版本条件。
  const instance = useDetailInstance(kind, target);
  useEffect(() => {
    setMcVersion(kind === "modpack" ? "" : instance.mcVersion);
    setLoader(
      kind !== "modpack" && RETRIEVABLE_LOADERS.has(instance.loader)
        ? instance.loader
        : ""
    );
  }, [kind, instance]);

  // 远端版本清单（跟随全局下载源）；拿不到时下拉退化为主版本之外的自由输入，不报错。
  useEffect(() => {
    let cancelled = false;
    listRemoteVersions(settings.versionListSource)
      .then((manifest) => {
        if (cancelled) return;
        setVersionIds(manifest.versions.map((version) => version.id));
      })
      .catch(() => {
        /* 清单不可用：下拉仍支持自由输入 */
      });
    return () => {
      cancelled = true;
    };
  }, [settings.versionListSource]);

  // 版本归「全部」时加载器同步清空（值联动保留；渐进式浮现动画已废弃）。
  const handleVersionChange = useCallback((next: string) => {
    setMcVersion(next);
    if (!next) setLoader("");
  }, []);

  const handleSourceChange = useCallback((next: string) => {
    setSource(next as CommunitySource);
    setPage(1);
  }, []);

  // 检索条件与分类变化一律回第 1 页；query 由引擎 hook 自行防抖。
  useEffect(() => {
    setPage(1);
  }, [kind, query, mcVersion, loader]);

  // 传给引擎的过滤条件：字段空串 = 该维度不过滤。
  const filter: ModrinthFilter = useMemo(
    () => ({ mcVersion, loader }),
    [mcVersion, loader]
  );

  const listAnchorRef = useRef<HTMLDivElement | null>(null);
  const scrollTimer = useRef<number | null>(null);
  const scrollFrame = useRef<number | null>(null);
  const scrollKey = useRef("");
  const cancelScroll = useCallback(() => {
    if (scrollTimer.current !== null) window.clearTimeout(scrollTimer.current);
    if (scrollFrame.current !== null)
      window.cancelAnimationFrame(scrollFrame.current);
    scrollTimer.current = null;
    scrollFrame.current = null;
  }, []);
  useEffect(() => {
    const target =
      listAnchorRef.current?.closest<HTMLElement>(".ni-pager__page");
    target?.addEventListener("resource-scroll-cancel", cancelScroll);
    return () =>
      target?.removeEventListener("resource-scroll-cancel", cancelScroll);
  }, [cancelScroll]);
  const scrollToTop = useCallback(() => {
    cancelScroll();
    const reduced = window.matchMedia(
      "(prefers-reduced-motion: reduce)"
    ).matches;
    scrollTimer.current = window.setTimeout(() => {
      scrollTimer.current = null;
      const anchor = listAnchorRef.current;
      if (!anchor || anchor.getClientRects().length === 0) return;
      const scroller = anchor.closest<HTMLElement>(".ni-pager__page");
      if (!scroller) return;
      if (reduced) {
        scroller.scrollTop = 0;
        return;
      }
      const start = scroller.scrollTop;
      const startedAt = performance.now();
      const step = (now: number) => {
        const progress = Math.min(1, (now - startedAt) / 220);
        scroller.scrollTop = start * Math.pow(1 - progress, 3);
        scrollFrame.current =
          progress < 1 ? window.requestAnimationFrame(step) : null;
      };
      scrollFrame.current = window.requestAnimationFrame(step);
    }, 80);
  }, [cancelScroll]);
  useLayoutEffect(() => {
    const key = `${page}:${kind}`;
    // Resuming the retained search page is not pagination; never erase its scroll.
    if (active && scrollKey.current !== key) {
      scrollKey.current = key;
      scrollToTop();
    }
    return cancelScroll;
  }, [active, page, kind, scrollToTop, cancelScroll]);

  // 三种来源各持一个检索 hook（hooks 顺序稳定），但只启用当前来源的那个，
  // 未启用的 hook 不发请求；同一时刻只消费被启用 hook 的返回值。
  const combined = useCombinedSearch({
    kind,
    query,
    filter,
    page,
    enabled: source === "all",
  });
  const modrinth = useModrinthSearch(
    kind,
    query,
    filter,
    page,
    source === "modrinth"
  );
  const curseforge = useCurseforgeSearch(
    kind,
    query,
    filter,
    page,
    source === "curseforge"
  );

  const view = useMemo(() => {
    if (source === "all") return combined;
    if (source === "modrinth") return modrinth;
    return curseforge;
  }, [source, combined, modrinth, curseforge]);

  const goPrev = useCallback(() => setPage((p) => Math.max(1, p - 1)), []);
  const goNext = useCallback(() => setPage((p) => p + 1), []);

  const isLoading = view.state === "loading";

  return (
    <div className={`mr-page${isLoading ? " mr-page--loading" : ""}`}>
      <header className="cv-page__head">
        <h2 className="cv-page__title">{label}</h2>

        <div className="cv-toolbar">
          <label className="mr-search cv-toolbar__search">
            <IconSearch aria-hidden />
            <input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={`搜索${label}`}
              aria-label={`搜索${label}`}
            />
          </label>

          <McVersionCombobox
            versions={versionIds}
            value={mcVersion}
            onChange={handleVersionChange}
            emptyText={
              versionIds.length > 0
                ? "没有匹配的版本号"
                : "版本清单不可用，可直接输入版本号"
            }
            ariaLabel="MC 版本"
            className="cv-toolbar__version"
          />

          {kind === "mod" ? (
            <LoaderSelect
              value={loader}
              options={LOADER_OPTIONS}
              onChange={setLoader}
            />
          ) : null}

          <Select
            size="sm"
            className="cv-toolbar__source"
            ariaLabel="检索来源"
            value={source}
            onChange={handleSourceChange}
            options={SOURCE_OPTIONS.map((option) => ({
              value: option.value,
              label: option.label,
            }))}
          />
        </div>
      </header>

      <>
        {/* 翻页锚点：纯空 div。 */}
        <div ref={listAnchorRef} aria-hidden="true" />

        {source === "all" ? (
          <ModrinthSearchPage
            kind={kind}
            view={combined}
            loadingLabel="正在检索"
            onOpen={onOpenDetail}
          />
        ) : source === "modrinth" ? (
          <ModrinthSearchPage
            kind={kind}
            view={modrinth}
            onOpen={onOpenDetail}
          />
        ) : (
          <CurseforgeSearchPage
            kind={kind}
            view={curseforge}
            onOpen={onOpenDetail}
          />
        )}

        {view.state === "ready" && (page > 1 || view.hasMore) ? (
          <CommunityPagination
            page={page}
            hasMore={view.hasMore}
            onFirst={() => setPage(1)}
            onPrevious={goPrev}
            onNext={goNext}
          />
        ) : null}
      </>
    </div>
  );
}
