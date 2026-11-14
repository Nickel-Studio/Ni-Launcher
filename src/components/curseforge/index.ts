/**
 * CurseForge 社区资源 · 对外入口。
 *
 * 页面侧（Resources.tsx「社区资源」分支的来源切换）只消费这里的导出：
 * - `CurseforgeSearchPage`  分类独立页（搜索 + 结果列表，点卡进独立详情页）
 * - `fetchCurseforgeConfigured`  引擎配置状态（key 只在后端使用，这里只拿布尔值）
 */
export { CurseforgeSearchPage } from "./CurseforgeSearchPage";
export { fetchCurseforgeConfigured } from "./ipc";
export {
  useCurseforgeSearch,
  PAGE_SIZE,
  type SearchState,
} from "./useCurseforgeSearch";
export { type CurseforgeKind, type CurseforgeFilter } from "./types";
