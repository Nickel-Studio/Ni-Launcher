/**
 * CurseForge 社区资源 · 检索状态 hook。
 *
 * 与 `useModrinthSearch` 同构：过滤条件由工具栏（props）驱动 → 搜索 → 分页。
 * 差异点只有两个：
 * 1. 数据源走 `curseforge_search`（后端已归一化为 Modrinth 同形结构）；
 * 2. loader 维度只接受 forge / neoforge / fabric（CF 的 ModLoaderType 白名单），
 *    optifine 实例与 Modrinth 一样检索不可用，由页面给出可执行提示。
 * 分页由页面驱动（`page` prop，1 起计）：整页拉取替代旧的无限滚动，
 * 切换筛选 / 翻页由 CommunitySearchPage 统一控制。`enabled = false` 时
 * 完全不发请求（页面同时挂着多个来源的 hook，只有当前来源被启用）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { searchCurseforge } from "./ipc";
import type { CurseforgeFilter, CurseforgeKind } from "./types";
import type { ModrinthHit } from "../modrinth/types";

/** 列表页状态机：`blocked` = 当前引擎在该实例下不可检索。 */
export type SearchState = "loading" | "ready" | "empty" | "error" | "blocked";

/** 每页条数（CF 官方 pageSize 上限 50，与 Modrinth 每页对齐）。 */
export const PAGE_SIZE = 20;

export interface UseCurseforgeSearch {
  state: SearchState;
  /** 失败 / 空态的具体原因（可执行的中文提示）。 */
  message: string;
  hits: ModrinthHit[];
  /** 命中总数（来自 CurseForge 的真实 total）。 */
  total: number;
  /** 是否还有下一页（由真实 total 推导）。 */
  hasMore: boolean;
  /** 按当前条件重新检索当前页（错误态重试 / 手动刷新）。 */
  reload: () => void;
}

export function useCurseforgeSearch(
  kind: CurseforgeKind,
  query: string,
  filter: CurseforgeFilter,
  page: number,
  enabled = true
): UseCurseforgeSearch {
  const [state, setState] = useState<SearchState>("loading");
  const [message, setMessage] = useState("");
  const [hits, setHits] = useState<ModrinthHit[]>([]);
  const [total, setTotal] = useState(0);
  const [nonce, setNonce] = useState(0);

  // 防抖：搜索框输入停顿 350ms 后才发起请求（与 Modrinth 页一致）。
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
      setTotal(0);

      const gameVersion = filter.mcVersion.trim();
      const loaderQuery = filter.loader.trim();
      if (gameVersion && loaderQuery === "optifine") {
        // 真实限制：CurseForge 没有 OptiFine 生态，按版本过滤检索必然 0 结果。
        setState("blocked");
        setMessage(
          "该实例为 OptiFine，CurseForge 上没有对应的 Mod 生态；把加载器换成 Fabric / Forge 即可继续检索，或先清空加载器浏览全站。"
        );
        return;
      }

      try {
        const result = await searchCurseforge({
          kind,
          query: debouncedQuery,
          gameVersion,
          loader: loaderQuery,
          offset: (page - 1) * PAGE_SIZE,
          limit: PAGE_SIZE,
        });
        if (cancelled) return;
        setHits(result.hits);
        setTotal(result.total);
        setState(result.hits.length > 0 ? "ready" : "empty");
        if (result.hits.length === 0) {
          setMessage(
            debouncedQuery
              ? "没有匹配的检索结果，试试换个关键词，或调整版本 / 加载器筛选后再试。"
              : "该分类下没有适配当前筛选条件的资源，试试放宽版本或加载器，或换个分类。"
          );
        }
      } catch (err) {
        if (cancelled) return;
        setState("error");
        setMessage(err instanceof Error ? err.message : String(err));
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

  return {
    state,
    message,
    hits,
    total,
    hasMore: page * PAGE_SIZE < total,
    reload,
  };
}
