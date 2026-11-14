/**
 * 社区资源 · 对外入口。
 *
 * 页面侧（Resources.tsx「社区资源」分支）只消费这里的导出：
 * - `CommunitySearchPage`  单页头 + 单工具栏的分类检索页
 * - `McVersionCombobox`    可输入的 MC 版本下拉（工具栏用）
 * - `ProjectDetailPage`    独立项目详情页（替换旧的内嵌展开区）
 */
export { CommunitySearchPage, type CommunitySource } from "./CommunitySearchPage";
export { McVersionCombobox } from "./McVersionCombobox";
export { ProjectDetailPage } from "./ProjectDetailPage";
