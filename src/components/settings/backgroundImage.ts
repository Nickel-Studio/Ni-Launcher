import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";

/** 支持的背景图片扩展名 */
export const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"];

/**
 * 通过 Tauri 对话框插件选择背景图片。
 * 直接调用插件命令 `plugin:dialog|open`（等价于 @tauri-apps/plugin-dialog 的 open()），
 * 以避免引入额外的 npm 依赖；插件/环境不可用时抛错，由调用方回退到 <input type="file">。
 *
 * @returns 原始文件路径；用户取消返回 null。
 */
export async function pickImagePath(): Promise<string | null> {
  if (!isTauri()) throw new Error("not-in-tauri");

  const selected = await invoke<string | string[] | null>("plugin:dialog|open", {
    options: {
      multiple: false,
      directory: false,
      title: "选择背景图片",
      filters: [{ name: "图片", extensions: IMAGE_EXTENSIONS }],
    },
  });

  if (!selected) return null;
  return Array.isArray(selected) ? (selected[0] ?? null) : selected;
}

/** 把 <input type="file"> 选中的文件读取为 dataURL。 */
export function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error("read-failed"));
    reader.readAsDataURL(file);
  });
}
