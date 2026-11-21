import steveHead from "../../assets/skins/steve-head.png";
import alexHead from "../../assets/skins/alex-head.png";

/** 内置离线兜底头像（随包体发布，最终兜底绝不依赖网络）。 */
export const OFFLINE_HEADS = [steveHead, alexHead] as const;

/** 随机取一个内置兜底头像（Steve / Alex 二选一）。 */
export function randomOfflineHead(): string {
  return (
    OFFLINE_HEADS[Math.floor(Math.random() * OFFLINE_HEADS.length)] ?? steveHead
  );
}

/** mc-heads 远程头像地址；名字为空时退化为 steve。 */
export function mcHeadsUrl(name: string, size = 160): string {
  const safe = (name || "steve").trim() || "steve";
  return `https://mc-heads.net/avatar/${encodeURIComponent(safe)}/${size}`;
}

/* --------------------------------------------------------------- 本地缓存 */
const CACHE_PREFIX = "nickel.avatar.";

function cacheKey(name: string): string {
  const key = (name || "").trim().toLowerCase();
  return `${CACHE_PREFIX}${key || "__default__"}`;
}

/** 读取本地缓存的头像 dataURL（无则返回 null）。 */
export function readAvatarCache(name: string): string | null {
  try {
    const value = window.localStorage.getItem(cacheKey(name));
    return value && value.startsWith("data:") ? value : null;
  } catch {
    return null;
  }
}

/** 写入本地缓存的头像 dataURL。 */
export function writeAvatarCache(name: string, dataUrl: string): void {
  if (!dataUrl || !dataUrl.startsWith("data:")) return;
  try {
    window.localStorage.setItem(cacheKey(name), dataUrl);
  } catch {
    /* 存储禁用 / 超额：忽略，不影响主流程 */
  }
}

/** 移除某个名字的本地头像缓存。 */
export function clearAvatarCache(name: string): void {
  try {
    window.localStorage.removeItem(cacheKey(name));
  } catch {
    /* ignore */
  }
}

/* ----------------------------------------------------------- 皮肤裁切 API */
function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("图片加载失败"));
    img.src = src;
  });
}

/** 脸与帽层共用透明画布，帽层外扩；比例需与满幅头像的 CSS 缩放保持一致。 */
const FACE_RATIO = 48 / 64;
const HAT_RATIO = 56 / 64;

function headBox(
  size: number,
  ratio: number
): { x: number; y: number; edge: number } {
  const edge = size * ratio;
  const offset = (size - edge) / 2;
  return { x: offset, y: offset, edge };
}

/** 区域内是否存在不透明像素（用来跳过空的帽层）。 */
function hasOpaque(
  img: HTMLImageElement,
  sx: number,
  sy: number,
  edge: number
): boolean {
  const probe = document.createElement("canvas");
  probe.width = edge;
  probe.height = edge;
  const ctx = probe.getContext("2d", { willReadFrequently: true });
  if (!ctx) return false;
  ctx.drawImage(img, sx, sy, edge, edge, 0, 0, edge, edge);
  const data = ctx.getImageData(0, 0, edge, edge).data;
  for (let index = 3; index < data.length; index += 4) {
    if (data[index] > 0) return true;
  }
  return false;
}

/**
 * 合成头像：输出一张带透明边距的方块图，最近邻放大。
 *
 * - `skin`：64×64（含 128×128 高清）皮肤 → 脸部 8×8@(8,8) + 帽层 8×8@(40,8)；
 *   只有 64×64 以上才有真正的第二层，64×32 旧皮肤不画帽层（那块区域是腿部镜像，
 *   画上去会变成一顶黑帽子）。
 * - `avatar`：已经是方块头像（皮肤站渲染图 / 本机任意图片）→ 直接铺满脸部区域，
 *   不裁切，避免把脸放大到失真。
 */
export async function composeHead(
  dataUrl: string,
  size = 256,
  mode: "skin" | "avatar" = "skin"
): Promise<string> {
  const img = await loadImage(dataUrl);
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("无法创建 2D 画布");

  ctx.imageSmoothingEnabled = false;
  const face = headBox(size, FACE_RATIO);
  // 每个「64 格」对应的图片像素：128×128 高清皮肤按 2 倍取样。
  const unit = Math.max(1, Math.floor(img.naturalWidth / 64));

  if (mode === "skin" && img.naturalWidth >= 64 && img.naturalHeight >= 32) {
    // 只取脸部的 8×8 区域；64×32 旧皮肤也走这里，绝不能把整张图铺到脸上。
    ctx.drawImage(
      img,
      8 * unit,
      8 * unit,
      8 * unit,
      8 * unit,
      face.x,
      face.y,
      face.edge,
      face.edge
    );
    // 帽层只有 64 高及以上的皮肤才有；64×32 的 (40,8) 区域是腿部镜像。
    if (
      img.naturalHeight >= 64 &&
      hasOpaque(img, 40 * unit, 8 * unit, 8 * unit)
    ) {
      const hat = headBox(size, HAT_RATIO);
      ctx.drawImage(
        img,
        40 * unit,
        8 * unit,
        8 * unit,
        8 * unit,
        hat.x,
        hat.y,
        hat.edge,
        hat.edge
      );
    }
  } else {
    // 非标准皮肤图 / 已渲染好的方块头像：居中铺满脸部区域。
    const edge = Math.min(img.naturalWidth, img.naturalHeight) || 1;
    ctx.drawImage(img, 0, 0, edge, edge, face.x, face.y, face.edge, face.edge);
  }

  return canvas.toDataURL("image/png");
}

/** 读取本地文件为 dataURL。 */
export function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error("文件读取失败"));
    reader.readAsDataURL(file);
  });
}
