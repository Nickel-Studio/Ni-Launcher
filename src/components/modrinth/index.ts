/**
 * Modrinth 社区资源 · 对外入口。
 *
 * 页面侧（Resources.tsx「社区资源」分支）只消费这里的导出：
 * - `ModrinthSearchPage`  分类独立页（搜索 + 结果列表，点卡进独立详情页）
 * - `MODRINTH_CATEGORIES` 分类定义（侧边栏项 / 安装目录映射）
 * - `useModrinthSearch`   检索状态 hook
 */
export {
  MODRINTH_CATEGORIES,
  type ModrinthKind,
  type ModrinthCategoryDef,
  type ModrinthFilter,
  type ModrinthHit,
} from "./types";
export {
  useModrinthSearch,
  PAGE_SIZE,
  type SearchState,
} from "./useModrinthSearch";
export { ModrinthSearchPage } from "./ModrinthSearchPage";
export { formatDownloads, formatIsoDate } from "./parse";
