/**
 * Modrinth 社区资源 · 检索状态 hook。
 *
 * 只负责「列表页」这一层：过滤条件由工具栏（props）驱动 → 搜索 → 分页。
 * `gameVersion` / `loader` 为空串表示该维度不过滤。debounce 350ms 保留；
 * 失败 / 超时 / 无结果都收敛为明确状态，由页面据此展示可执行的中文提示。
 * 分页由页面驱动（`page` prop，1 起计）：整页拉取替代旧的无限滚动，
 * 切换筛选 / 翻页由 CommunitySearchPage 统一控制。`enabled = false` 时
 * 完全不发请求（页面同时挂着多个来源的 hook，只有当前来源被启用）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { searchModrinth } from "./ipc";
import type { ModrinthFilter, ModrinthHit, ModrinthKind } from "./types";

/** 列表页状态机：`blocked` = 当前引擎在该实例下不可检索。 */
export type SearchState = "loading" | "ready" | "empty" | "error" | "blocked";

/** 每页条数（Modrinth 上限 100，与 CurseForge 每页对齐）。 */
export const PAGE_SIZE = 20;

export interface UseModrinthSearch {
  state: SearchState;
  /** 失败 / 空态的具体原因（可执行的中文提示）。 */
  message: string;
  hits: ModrinthHit[];
  /** 命中总数（来自 Modrinth 的真实 total）。 */
  total: number;
  /** 是否还有下一页（由真实 total 推导）。 */
  hasMore: boolean;
  /** 按当前条件重新检索当前页（错误态重试 / 手动刷新）。 */
  reload: () => void;
}

export function useModrinthSearch(
  kind: ModrinthKind,
  query: string,
  filter: ModrinthFilter,
  page: number,
  enabled = true
): UseModrinthSearch {
  const [state, setState] = useState<SearchState>("loading");
  const [message, setMessage] = useState("");
  const [hits, setHits] = useState<ModrinthHit[]>([]);
  const [total, setTotal] = useState(0);
  const [nonce, setNonce] = useState(0);

  // 防抖：搜索框输入停顿 350ms 后才发起请求。
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
        // 真实限制：Modrinth 没有 OptiFine 生态，按版本过滤检索必然 0 结果。
        setState("blocked");
        setMessage(
          "该实例为 OptiFine，Modrinth 上没有对应的 Mod 生态；把加载器换成 Fabric / Forge 即可继续检索，或先清空加载器浏览全站。"
        );
        return;
      }

      try {
        const result = await searchModrinth({
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
