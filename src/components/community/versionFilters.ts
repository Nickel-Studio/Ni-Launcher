/**
 * 只按真实 Minecraft 版本在本地筛选，完整历史不被截断。
 * 加载器分组必须合并项目声明与文件实际标记；CurseForge 项目声明可能缺失。
 * 大版本筛选只聚合选项，卡片仍按精确版本与加载器拆分。
 */
import type { ModrinthKind, ModrinthVersion } from "../modrinth/types";

export const ALL_FILTER = "全部";
/** 无可用版本标记的文件归入的卡（仅「全部」筛选下显示）。 */
export const OTHER_VERSIONS = "其他版本";
export const SNAPSHOT_LABEL = "快照版";
export const LEGACY_LABEL = "远古版";
const GROUP_THRESHOLD = 9;

/** 正式版：1.20 / 1.20.1 / 26.2 这类纯数字版本号。 */
const RELEASE_RE = /^\d+\.\d+(?:\.\d+)?$/;
/** 远古形态：Beta / Alpha / Classic / Indev / rd- 预经典版。 */
const LEGACY_RE = /^(?:b1\.|a1\.|c0\.|rd-|inf-)/;

/**
 * gameVersions 条目分类。
 * `other` = 引擎混入的非版本标记（Forge、Fabric、Java 8…），不参与分组。
 */
export function classifyGameVersion(
  value: string
): "release" | "snapshot" | "legacy" | "other" {
  const tag = value.trim();
  if (RELEASE_RE.test(tag)) return "release";
  if (LEGACY_RE.test(tag)) return "legacy";
  if (/^\d/.test(tag)) return "snapshot";
  return "other";
}

/** 旧 1.x 版本按第二段编码；26.x 起保留 major 与 minor，非正式版返回 null。 */
export function versionToDrop(tag: string): number | null {
  const trimmed = tag.trim();
  if (classifyGameVersion(trimmed) !== "release") return null;
  const parts = trimmed.split(".").map(Number);
  const major = parts[0];
  const minor = parts[1] ?? 0;
  if (!Number.isFinite(major) || !Number.isFinite(minor)) return null;
  return major === 1 ? minor * 10 : major * 10 + minor;
}

export function dropToVersion(drop: number): string {
  return drop >= 250
    ? `${Math.floor(drop / 10)}.${drop % 10}`
    : `1.${Math.floor(drop / 10)}`;
}

/** 远古聚合以 1.20 为界，其 Drop 编码为 200。 */
const FOLD_OLD_DROP = 200;

/** 版本格式是否原版合法（^1.\d 或 ≥26.x），加载器拆分的前提。 */
export function isFormatFit(tag: string): boolean {
  const trimmed = tag.trim();
  if (/^1\.\d/.test(trimmed)) return true;
  const match = /^([2-9]\d)\.\d+/.exec(trimmed);
  return match !== null && Number(match[1]) >= 26;
}

/** 非版本标记返回 OTHER_VERSIONS，由调用方剔除。 */
export function groupedVersionName(
  value: string,
  groupedByDrop: boolean,
  foldOld: boolean
): string {
  const tag = value.trim();
  switch (classifyGameVersion(tag)) {
    case "other":
      return OTHER_VERSIONS;
    case "snapshot":
      return SNAPSHOT_LABEL;
    case "legacy":
      return LEGACY_LABEL;
    case "release": {
      const drop = versionToDrop(tag);
      if (drop === null) return OTHER_VERSIONS;
      if (foldOld && drop < FOLD_OLD_DROP) return LEGACY_LABEL;
      return groupedByDrop ? dropToVersion(drop) : tag;
    }
  }
}

/** 版本号感知的倒序比较（1.21 > 1.9；26.2 > 1.21）。 */
function compareVersionDesc(a: string, b: string): number {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i += 1) {
    const x = pa[i] ?? 0;
    const y = pb[i] ?? 0;
    if (x !== y) return y - x;
  }
  return 0;
}

/** 组名排序：正式版组降序在前，特殊组（快照版 / 远古版）垫底。 */
function sortGroupNames(names: readonly string[]): string[] {
  const releases = names
    .filter((name) => /^\d/.test(name))
    .sort(compareVersionDesc);
  const specials = names
    .filter((name) => !/^\d/.test(name))
    .sort((a, b) => a.localeCompare(b, "zh"));
  return [...releases, ...specials];
}

/** pill 推导结果：分组名（不含「全部」）+ 当前降级层级。 */
export interface VersionFilterState {
  pills: string[];
  groupedByDrop: boolean;
  foldOld: boolean;
}

/** 返回值不含「全部」；依次聚合大版本、远古版本，直至选项数小于阈值。 */
export function deriveVersionPills(
  gameVersions: readonly string[]
): VersionFilterState {
  const levels: ReadonlyArray<readonly [boolean, boolean]> = [
    [false, false],
    [true, false],
    [false, true],
    [true, true],
  ];
  let state: VersionFilterState = {
    pills: [],
    groupedByDrop: false,
    foldOld: false,
  };
  for (const [groupedByDrop, foldOld] of levels) {
    const names = new Set<string>();
    for (const value of gameVersions) {
      const name = groupedVersionName(value, groupedByDrop, foldOld);
      if (name !== OTHER_VERSIONS) names.add(name);
    }
    state = { pills: sortGroupNames([...names]), groupedByDrop, foldOld };
    if (state.pills.length < GROUP_THRESHOLD) break;
  }
  return state;
}

/** 只有文件精确支持实例版本时，才默认选中对应分组。 */
export function defaultFilterFor(
  state: VersionFilterState,
  instanceVersion: string,
  supported: boolean
): string {
  const trimmed = instanceVersion.trim();
  if (!supported || !trimmed) return ALL_FILTER;
  const group = groupedVersionName(trimmed, state.groupedByDrop, state.foldOld);
  return state.pills.includes(group) ? group : ALL_FILTER;
}

/** 版本行是否命中筛选（前端本地过滤，不重新请求）。 */
export function versionMatchesFilter(
  gameVersions: readonly string[],
  filter: string,
  groupedByDrop: boolean,
  foldOld: boolean
): boolean {
  if (filter === ALL_FILTER) return true;
  return gameVersions.some(
    (value) => groupedVersionName(value, groupedByDrop, foldOld) === filter
  );
}

/** 加载器标识 → 展示名（分组卡标题前缀）。 */
const LOADER_LABELS: Record<string, string> = {
  forge: "Forge",
  neoforge: "NeoForge",
  fabric: "Fabric",
  quilt: "Quilt",
  liteloader: "LiteLoader",
  optifine: "OptiFine",
};
export function loaderLabel(loader: string): string {
  const key = loader.trim().toLowerCase();
  return LOADER_LABELS[key] ?? loader.trim();
}

/** 是否为已知加载器（vanilla / 空串不算）。 */
function knownLoader(loader: string): boolean {
  return Object.prototype.hasOwnProperty.call(
    LOADER_LABELS,
    loader.trim().toLowerCase()
  );
}

/** 同版本卡片使用确定的加载器顺序，避免 API 返回顺序影响浏览。 */
const LOADER_ORDER: Record<string, number> = {
  forge: 0,
  neoforge: 1,
  fabric: 2,
  quilt: 3,
  liteloader: 4,
  optifine: 5,
};
function loaderRank(loader: string): number {
  if (!loader) return 99;
  return LOADER_ORDER[loader.trim().toLowerCase()] ?? 50;
}

/** 一张分组卡（标题即唯一键）。 */
export interface VersionGroup {
  key: string;
  title: string;
  /** 卡内版本（发布时间降序、按 id 去重）。 */
  items: ModrinthVersion[];
  /** 「所选版本」置顶卡。 */
  selected: boolean;
  /** 标题已含加载器前缀（版本行不再重复显示加载器 chip）。 */
  hasLoaderPrefix: boolean;
  /** 「其他版本」卡：显示版本收录延迟提示。 */
  other: boolean;
}

/** 所选版本卡替代同名普通卡；同一文件跨版本分卡，单卡内按 id 去重。 */
export function buildVersionGroups(params: {
  versions: readonly ModrinthVersion[];
  kind: ModrinthKind;
  projectLoaders: readonly string[];
  filter: string;
  groupedByDrop: boolean;
  foldOld: boolean;
  instanceVersion: string;
  instanceLoader: string;
}): VersionGroup[] {
  const {
    versions,
    kind,
    projectLoaders,
    filter,
    groupedByDrop,
    foldOld,
    instanceVersion,
    instanceLoader,
  } = params;
  const sorted = [...versions].sort(
    (a, b) =>
      (Date.parse(b.datePublished) || 0) - (Date.parse(a.datePublished) || 0)
  );
  // 拆分依据 = 项目声明 ∪ 文件实际加载器：CurseForge 命中常缺 loaders，
  // 只看项目声明会把 Forge/NeoForge/Fabric 揉进同一版本号卡。
  const loaderUniverse = new Set(
    [...projectLoaders, ...versions.flatMap((version) => version.loaders)]
      .map((loader) => loader.trim().toLowerCase())
      .filter((loader) => knownLoader(loader))
  );
  const splitByLoader = kind === "mod" && loaderUniverse.size >= 2;

  const cards = new Map<
    string,
    { items: ModrinthVersion[]; hasLoaderPrefix: boolean; other: boolean }
  >();
  const addToCard = (
    key: string,
    version: ModrinthVersion,
    hasLoaderPrefix: boolean,
    other: boolean
  ) => {
    const card = cards.get(key) ?? { items: [], hasLoaderPrefix, other };
    if (!card.items.some((item) => item.id === version.id)) {
      card.items.push(version);
    }
    cards.set(key, card);
  };

  for (const version of sorted) {
    const tags = [...new Set(version.gameVersions)].filter(
      (tag) => classifyGameVersion(tag) !== "other"
    );
    if (tags.length === 0) {
      if (filter === ALL_FILTER)
        addToCard(OTHER_VERSIONS, version, false, true);
      continue;
    }
    const versionLoaders = [
      ...new Set(
        version.loaders
          .map((loader) => loader.trim().toLowerCase())
          .filter((loader) => knownLoader(loader))
      ),
    ];
    for (const tag of tags) {
      if (
        filter !== ALL_FILTER &&
        groupedVersionName(tag, groupedByDrop, foldOld) !== filter
      ) {
        continue;
      }
      const verName = groupedVersionName(tag, false, false);
      const prefixes =
        splitByLoader && isFormatFit(verName) && versionLoaders.length > 0
          ? versionLoaders.map((loader) => loaderLabel(loader))
          : [""];
      for (const prefix of prefixes) {
        const key = prefix ? `${prefix} ${verName}` : verName;
        addToCard(key, version, prefix !== "", false);
      }
    }
  }

  // 「所选版本」置顶卡：精确支持实例 mcVersion（加载器交集非空）的文件集。
  const instanceKey = instanceLoader.trim().toLowerCase();
  const instancePrefix = knownLoader(instanceKey)
    ? loaderLabel(instanceKey)
    : "";
  const trimmedInstanceVersion = instanceVersion.trim();
  let selectedTitle = "";
  if (trimmedInstanceVersion) {
    selectedTitle = `${
      instancePrefix ? instancePrefix + " " : ""
    }${trimmedInstanceVersion}（所选版本）`;
    const items = sorted.filter(
      (version) =>
        version.gameVersions.includes(trimmedInstanceVersion) &&
        (!instancePrefix ||
          version.loaders.some(
            (loader) => loader.trim().toLowerCase() === instanceKey
          )) &&
        versionMatchesFilter(
          version.gameVersions,
          filter,
          groupedByDrop,
          foldOld
        )
    );
    if (items.length > 0) {
      cards.set(selectedTitle, {
        items,
        hasLoaderPrefix: instancePrefix !== "",
        other: false,
      });
      cards.delete(selectedTitle.replace("（所选版本）", ""));
    }
  }

  const groups: VersionGroup[] = [...cards].map(([title, card]) => ({
    key: title,
    title,
    items: card.items,
    selected: title === selectedTitle,
    hasLoaderPrefix: card.hasLoaderPrefix,
    other: card.other,
  }));
  // 置顶卡 > 版本号降序 > 同版本 Forge → NeoForge → Fabric > 特殊版本垫底。
  const splitTitle = (
    group: VersionGroup
  ): { loader: string; version: string } => {
    const withoutSuffix = group.title.replace("（所选版本）", "");
    const space = withoutSuffix.indexOf(" ");
    const head = space > 0 ? withoutSuffix.slice(0, space) : "";
    return knownLoader(head)
      ? { loader: head, version: withoutSuffix.slice(space + 1) }
      : { loader: "", version: withoutSuffix };
  };
  groups.sort((a, b) => {
    if (a.selected !== b.selected) return a.selected ? -1 : 1;
    const ta = splitTitle(a);
    const tb = splitTitle(b);
    const specialA = !ta.version.includes(".");
    const specialB = !tb.version.includes(".");
    if (specialA !== specialB) return specialA ? 1 : -1;
    if (specialA && specialB) return ta.version.localeCompare(tb.version, "zh");
    const order = compareVersionDesc(ta.version, tb.version);
    if (order !== 0) return order;
    const rank = loaderRank(ta.loader) - loaderRank(tb.loader);
    return rank !== 0 ? rank : a.title.localeCompare(b.title, "zh");
  });
  return groups;
}
