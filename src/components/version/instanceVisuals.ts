/**
 * 版本实例的展示映射：Blocks 图标资源 + 中文标签 + 路径折叠。
 *
 * 图标资源已入库（`src/assets/blocks/*.png`），这里把后端 `instance_scan` 返回的
 * `icon` 键（取值集合与后端枚举一字不差）映射到对应图片。
 * 未知键一律回退红石块：任何实例在界面上都必须有明确图标，不能出现空白。
 *
 * 这些路径 / 文案工具是为本模块就近实现的小函数：它们与资源中心的同名工具逻辑
 * 等价，但版本模块不应依赖资源扫描模块（两者由不同协作者维护），因此没有跨域引用。
 */
import Anvil from "../../assets/blocks/Anvil.png";
import CobbleStone from "../../assets/blocks/CobbleStone.png";
import CommandBlock from "../../assets/blocks/CommandBlock.png";
import Egg from "../../assets/blocks/Egg.png";
import Fabric from "../../assets/blocks/Fabric.png";
import GoldBlock from "../../assets/blocks/GoldBlock.png";
import Grass from "../../assets/blocks/Grass.png";
import GrassPath from "../../assets/blocks/GrassPath.png";
import NeoForge from "../../assets/blocks/NeoForge.png";
import OptiFabric from "../../assets/blocks/OptiFabric.png";
import RedstoneBlock from "../../assets/blocks/RedstoneBlock.png";
import RedstoneLampOff from "../../assets/blocks/RedstoneLampOff.png";
import RedstoneLampOn from "../../assets/blocks/RedstoneLampOn.png";
import { resolveBackgroundUrl } from "../../lib/settings";

/** `icon` 键 → Blocks 图片（原版 / 快照 / 旧版 / 各加载器 / 愚人节）。 */
const BLOCK_ICONS: Record<string, string> = {
  grass: Grass,
  command_block: CommandBlock,
  cobblestone: CobbleStone,
  anvil: Anvil,
  neoforge: NeoForge,
  fabric: Fabric,
  grass_path: GrassPath,
  egg: Egg,
  gold_block: GoldBlock,
  redstone_block: RedstoneBlock,
  redstone_lamp_on: RedstoneLampOn,
  redstone_lamp_off: RedstoneLampOff,
};

/** 图标缺失或无法识别时的兜底（与「无法解析」语义一致）。 */
export const FALLBACK_BLOCK_ICON: string = RedstoneBlock;

export function blockIconSrc(icon: string): string {
  const builtIn = BLOCK_ICONS[icon];
  if (typeof builtIn === "string") return builtIn;
  // 自定义图标由后端保存为图片原路径；只在展示边界转为 asset URL。
  return /[/\\:]/.test(icon)
    ? resolveBackgroundUrl(icon) ?? FALLBACK_BLOCK_ICON
    : FALLBACK_BLOCK_ICON;
}

/** OptiFabric 加载器的专属图标（后端 `optifabric` 键；检测不到专用图标时回退 Fabric）。 */
export function optifabricIconSrc(): string {
  return OptiFabric;
}

/**
 * 红石灯组：亮/灭一对，用于「下载进行中 / 下载完成」等状态语义的展示。
 * `redstone_lamp_on` / `redstone_lamp_off` 是合法的图标键（个性化选择器可选手动指定）。
 */
export function redstoneLampSrc(on: boolean): string {
  return on ? RedstoneLampOn : RedstoneLampOff;
}

/** 图标键是否为红石灯（两种状态）。 */
export function isRedstoneLamp(icon: string): boolean {
  return icon === "redstone_lamp_on" || icon === "redstone_lamp_off";
}

/** 红石灯键的取值集合（版本设置的个性化图标选择器使用）。 */
export const REDSTONE_LAMP_KEYS = {
  on: "redstone_lamp_on",
  off: "redstone_lamp_off",
} as const;

const LOADER_LABELS: Record<string, string> = {
  vanilla: "原版",
  forge: "Forge",
  neoforge: "NeoForge",
  fabric: "Fabric",
  optifine: "OptiFine",
  optifabric: "OptiFabric",
  liteloader: "LiteLoader",
  legacy: "旧版",
  unknown: "未知来源",
};

/** 加载器标识 → 中文徽标文案。 */
export function loaderLabel(loader: string): string {
  return LOADER_LABELS[loader] ?? loader ?? "未知来源";
}

const TYPE_LABELS: Record<string, string> = {
  release: "正式版",
  snapshot: "快照",
  old_beta: "远古 Beta",
  old_alpha: "远古 Alpha",
  unknown: "无法识别",
};

/** 版本类型 → 中文标签。 */
export function typeLabel(type: string): string {
  return TYPE_LABELS[type] ?? type ?? "无法识别";
}

const ICON_LABELS: Record<string, string> = {
  grass: "原版",
  command_block: "快照",
  cobblestone: "旧版",
  anvil: "Forge",
  neoforge: "NeoForge",
  fabric: "Fabric",
  grass_path: "OptiFine",
  egg: "LiteLoader",
  gold_block: "愚人节",
  redstone_block: "未识别",
  redstone_lamp_on: "红石灯（开）",
  redstone_lamp_off: "红石灯（关）",
};

/** 图标键 → 可读文案（卡片角标用）。 */
export function iconLabel(icon: string): string {
  return ICON_LABELS[icon] ?? (/[/\\:]/.test(icon) ? "自定义图标" : "未识别");
}

/** 取路径最后一段作为目录名（兼容 `\\` 与 `/`，忽略结尾分隔符）。 */
export function dirName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (index < 0) return trimmed;
  return trimmed.slice(index + 1) || trimmed;
}

/**
 * 实例目录的展示名：自定义标签优先；启动器自带目录在没有自定义标签时显示为
 * 「默认实例目录」；其余仍使用目录最后一段。物理路径与目录身份不受影响。
 */
export function rootDisplayName(
  dir: string,
  labels: Record<string, string>,
  defaultDir: string
): string {
  const custom = labels[dir]?.trim();
  if (custom) return custom;
  const normalised = dir.replace(/[\\/]+$/, "");
  const preset = defaultDir.replace(/[\\/]+$/, "");
  if (preset && normalised.toLowerCase() === preset.toLowerCase()) {
    return "默认实例目录";
  }
  return dirName(dir);
}

/**
 * 路径预览：过长时从尾部按「整段」拼装，超出预算的部分用前导 `…` 折叠。
 *
 * 为什么按整段而不是按字符数截断：`.minecraft` 根目录的名字几乎都一样，
 * 真正用来区分它们的是父级目录；按字符数截断会把父级目录切成半个词（`…oaming/`），
 * 反而更难辨认。完整路径始终由 Tooltip 给出。
 */
export function pathEllipsis(path: string, max: number): string {
  const normalised = path.replace(/[\\/]+$/, "");
  if (normalised.length <= max || max < 8) return normalised;

  const segments = normalised
    .split(/[\\/]/)
    .filter((segment) => segment.length > 0);
  const kept: string[] = [];
  let length = 0;
  for (let i = segments.length - 1; i >= 0; i -= 1) {
    const segment = segments[i];
    const added = segment.length + (kept.length > 0 ? 1 : 0);
    if (kept.length > 0 && length + added > max - 1) break;
    kept.unshift(segment);
    length += added;
  }
  return `…${kept.join("/")}`;
}

/** 版本 JSON 的 releaseTime（RFC3339）→ `YYYY-MM-DD`；无法解析时截断原文。 */
export function formatReleaseTime(raw: string | null): string {
  if (!raw) return "";
  const time = Date.parse(raw);
  if (!Number.isFinite(time)) return raw.slice(0, 10);
  const date = new Date(time);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate()
  )}`;
}
