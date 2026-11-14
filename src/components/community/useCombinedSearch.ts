/** 两平台同时检索；缺少 CF Key 仍由后端尝试免 Key 镜像。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { searchModrinth } from "../modrinth/ipc";
import { searchCurseforge } from "../curseforge/ipc";
import { mergeHits } from "../modrinth/parse";
import type {
  ModrinthFilter,
  ModrinthHit,
  ModrinthKind,
} from "../modrinth/types";
import type { CommunitySearchView } from "./searchView";

/** 合并模式下每个来源每页取的条数（两源合计 20 = 单源每页条数）。 */
const PER_SOURCE_PAGE_SIZE = 10;

/** 合并模式的检索入参（kind / 过滤 / 关键词与单源一致；page 从 1 起计）。 */
interface CombinedParams {
  kind: ModrinthKind;
  query: string;
  filter: ModrinthFilter;
  page: number;
  /** false 时完全不发请求（页面同时挂着多个来源的 hook）。 */
  enabled: boolean;
  /** 页面已知的 CF 配置状态；null = 未知（本轮自行补查），false = 确认未配置。 */
}

/** 单源一页的请求结果（failure = 该源本轮不可用）。 */
interface SourcePage {
  total: number | null;
  hits: ModrinthHit[];
}

export function useCombinedSearch({
  kind,
  query,
  filter,
  page,
  enabled,
}: CombinedParams): CommunitySearchView {
  const [state, setState] = useState<CommunitySearchView["state"]>("loading");
  const [message, setMessage] = useState("");
  const [hits, setHits] = useState<ModrinthHit[]>([]);
  const [knownTotal, setKnownTotal] = useState<number | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [nonce, setNonce] = useState(0);

  // 防抖：与两个单源 hook 一致，输入停顿 350ms 后才发起请求。
  const [debouncedQuery, setDebouncedQuery] = useState(query.trim());
  const queryTimer = useRef<number | null>(null);
  useEffect(() => {
    if (queryTimer.current !== null) window.clearTimeout(queryTimer.current);
    queryTimer.current = window.setTimeout(() => {
      queryTimer.current = null;
      setDebouncedQuery(query.trim());
    }, 350);
    return () => {
      if (queryTimer.current !== null) {
        window.clearTimeout(queryTimer.current);
        queryTimer.current = null;
      }
    };
  }, [query]);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;

    async function run(): Promise<void> {
      setState("loading");
      setMessage("");
      setHits([]);
      setKnownTotal(null);
      setHasMore(false);

      const gameVersion = filter.mcVersion.trim();
      const loaderQuery = filter.loader.trim();
      const offset = (page - 1) * PER_SOURCE_PAGE_SIZE;

      const modrinthTask: Promise<SourcePage> = searchModrinth({
        kind,
        query: debouncedQuery,
        gameVersion,
        loader: loaderQuery,
        offset,
        limit: PER_SOURCE_PAGE_SIZE,
      }).then(
        (result): SourcePage => ({ total: result.total, hits: result.hits }),
        (): SourcePage => ({ total: null, hits: [] })
      );

      const curseforgeTask: Promise<SourcePage> = searchCurseforge({
            kind,
            query: debouncedQuery,
            gameVersion,
            loader: loaderQuery,
            offset,
            limit: PER_SOURCE_PAGE_SIZE,
          }).then(
            (result): SourcePage => ({
              total: result.total,
              hits: result.hits,
            }),
            (): SourcePage => ({ total: null, hits: [] })
          );

      const [modrinthPage, curseforgePage] = await Promise.all([
        modrinthTask,
        curseforgeTask,
      ]);
      if (cancelled) return;

      const merged = mergeHits(modrinthPage.hits, curseforgePage.hits);
      const modrinthMore = modrinthPage.hits.length === PER_SOURCE_PAGE_SIZE;
      const curseforgeMore =
        curseforgePage.hits.length === PER_SOURCE_PAGE_SIZE;
      const totals = [modrinthPage.total, curseforgePage.total].filter(
        (value): value is number => value !== null
      );

      setHits(merged);
      setKnownTotal(
        totals.length > 0 ? totals.reduce((sum, value) => sum + value, 0) : null
      );
      setHasMore(modrinthMore || curseforgeMore);

      if (totals.length === 0) {
        setState("error");
        setMessage("社区检索失败，请检查网络或镜像设置后重试。");
      } else if (merged.length === 0) {
        setState("empty");
        setMessage(
          debouncedQuery
            ? "没有匹配的检索结果，试试换个关键词，或调整版本 / 加载器筛选后再试。"
            : "该分类下没有适配当前筛选条件的资源，试试放宽版本或加载器，或换个分类。"
        );
      } else {
        setState("ready");
      }
    }

    void run();
    return () => {
      cancelled = true;
    };
  }, [
    kind,
    debouncedQuery,
    filter.mcVersion,
    filter.loader,
    page,
    nonce,
    enabled,
  ]);

  return { state, message, hits, total: knownTotal, hasMore, reload };
}
