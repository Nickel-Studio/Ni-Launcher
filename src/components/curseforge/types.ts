/**
 * CurseForge 社区资源 · 数据模型。
 *
 * 与后端 `curseforge.rs` 的命令契约一字不差（命令返回归一化 JSON 文本，字段 camelCase）：
 * - `cf_config_status`          → `{"configured":true|false}`（绝不回传 key 本身）
 * - `curseforge_search`         → `{"total":n,"hits":[...]}` —— 与 `modrinth_search` 同形
 * - `curseforge_project_files`  → `{versions:[...],offset,nextOffset,hasMore,total}`
 * - `curseforge_install`        → taskId（进度经 `install-event`，与版本安装同源）
 * - `curseforge_open_project`   → 打开 curseforge.com 项目页
 *
 * 检索结果直接复用 Modrinth 的卡片 / 弹层组件（`ModrinthHit` / `ModrinthVersion`），
 * 因此这里不重复定义 hit / version 结构，只做类型别名与来源特有常量。
 */

import type { ModrinthKind, ModrinthFilter } from "../modrinth/types";

/** 后端契约的资源种类（与 `modrinth_search` 的 `kind` 取值集合一致）。 */
export type CurseforgeKind = ModrinthKind;

/** 实例过滤条件：结构与 Modrinth 完全一致（两个引擎共用同一份数据）。 */
export type CurseforgeFilter = ModrinthFilter;

/** 检索一页结果的入参。 */
export interface CurseforgeSearchParams {
  kind: CurseforgeKind;
  query: string;
  gameVersion: string;
  /** 加载器标识（vanilla / forge / neoforge / fabric / optifine / …）。 */
  loader: string;
  offset: number;
  limit: number;
}

/** 列出项目文件的入参。 */
export interface CurseforgeFilesParams {
  kind: CurseforgeKind;
  projectId: string;
  gameVersion: string;
  loader: string;
  offset?: number;
  limit?: number;
}

/** 安装入参。 */
export interface CurseforgeInstallParams {
  kind: CurseforgeKind;
  projectId: string;
  fileId: string;
  dir: string;
}
