/**
 * 远端版本的四个分组（正式版 / 预览版 / 远古版 / 愚人节版）。
 *
 * Mojang 版本清单只有 release / snapshot / old_beta / old_alpha 四种类型，
 * **没有**「愚人节」字段，因此这里用一份已知愚人节版本的 id 白名单归类。
 * 新增愚人节版本时只需在此追加 id（这是本文件唯一需要维护的数据）。
 */
import type { RemoteVersion } from "../download/types";

export type VersionGroupId = "release" | "snapshot" | "old" | "april";

export interface VersionGroupDef {
  id: VersionGroupId;
  label: string;
  /** Blocks 图标键（见 `components/version/instanceVisuals`）。 */
  icon: string;
}

/** 分组展示顺序。 */
export const VERSION_GROUPS: readonly VersionGroupDef[] = [
  { id: "release", label: "正式版", icon: "grass" },
  { id: "snapshot", label: "预览版", icon: "command_block" },
  { id: "old", label: "远古版", icon: "cobblestone" },
  { id: "april", label: "愚人节版", icon: "gold_block" },
];

/** 已知愚人节版本的 id 白名单。 */
const APRIL_FOOLS_IDS: ReadonlySet<string> = new Set([
  "15w14a",
  "1.RV-Pre1",
  "3D Shareware v1.34",
  "20w14infinite",
  "22w13oneblockatatime",
  "23w13a_or_b",
  "24w14potato",
  "25w14craftmine",
]);

/** 单个版本所属分组。 */
export function versionGroupOf(version: RemoteVersion): VersionGroupId {
  if (APRIL_FOOLS_IDS.has(version.id)) return "april";
  if (version.type === "release") return "release";
  if (version.type.startsWith("old_")) return "old";
  // snapshot 及无法识别的类型统一归入预览版（列表仍按时间倒序，不会丢条目）。
  return "snapshot";
}

/** 按分组切分并按发布时间倒序（无法解析时间时保留原始顺序）。 */
export function groupVersions(
  versions: readonly RemoteVersion[]
): Record<VersionGroupId, RemoteVersion[]> {
  const groups: Record<VersionGroupId, RemoteVersion[]> = {
    release: [],
    snapshot: [],
    old: [],
    april: [],
  };

  versions.forEach((version, index) => {
    groups[versionGroupOf(version)].push({ ...version, __index: index } as RemoteVersion & {
      __index: number;
    });
  });

  for (const id of Object.keys(groups) as VersionGroupId[]) {
    groups[id].sort((a, b) => {
      const timeA = Date.parse(a.releaseTime);
      const timeB = Date.parse(b.releaseTime);
      const validA = Number.isFinite(timeA);
      const validB = Number.isFinite(timeB);
      if (validA && validB && timeA !== timeB) return timeB - timeA;
      if (validA !== validB) return validA ? -1 : 1;
      return (
        ((a as { __index?: number }).__index ?? 0) -
        ((b as { __index?: number }).__index ?? 0)
      );
    });
  }

  return groups;
}
