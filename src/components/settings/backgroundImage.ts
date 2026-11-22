import { invoke } from "@tauri-apps/api/core";
import { backgroundKind, isTauri } from "../../lib/settings";

export const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"];
export const VIDEO_EXTENSIONS = ["mp4", "m4v", "webm", "ogv"];
export const BACKGROUND_ACCEPT = [...IMAGE_EXTENSIONS, ...VIDEO_EXTENSIONS]
  .map((extension) => `.${extension}`)
  .join(",");
// The data URL is persisted in localStorage (a few MiB per origin), so browser previews stay small.
const BROWSER_MAX_BYTES = 3 * 1024 * 1024;

/** Native media stays on disk; only validated paths are persisted. */
export async function pickBackgroundPath(): Promise<string | null> {
  if (!isTauri()) throw new Error("not-in-tauri");
  const selected = await invoke<string | string[] | null>("plugin:dialog|open", {
    options: {
      multiple: false,
      directory: false,
      title: "选择背景图片或视频",
      filters: [
        { name: "图片与视频", extensions: [...IMAGE_EXTENSIONS, ...VIDEO_EXTENSIONS] },
        { name: "图片", extensions: IMAGE_EXTENSIONS },
        { name: "视频", extensions: VIDEO_EXTENSIONS },
      ],
    },
  });
  const path = Array.isArray(selected) ? (selected[0] ?? null) : selected;
  if (path) await invoke("background_media_validate", { path });
  return path;
}

/** Browser previews use bounded data URLs; desktop videos use range requests instead. */
export function readFileAsDataUrl(file: File): Promise<string> {
  const extension = file.name.split(".").pop()?.toLowerCase() ?? "";
  if (![...IMAGE_EXTENSIONS, ...VIDEO_EXTENSIONS].includes(extension))
    return Promise.reject(new Error("请选择支持的图片或视频文件。"));
  if (!file.size || file.size > BROWSER_MAX_BYTES)
    return Promise.reject(new Error("浏览器预览请选择不超过 3 MiB 的文件。"));
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const data = String(reader.result);
      const kind = backgroundKind(file.name);
      if (!data.startsWith(`data:${kind}/`)) {
        reject(new Error("无法识别该文件的媒体类型，请更换文件。"));
        return;
      }
      resolve(data);
    };
    reader.onerror = () => reject(reader.error ?? new Error("read-failed"));
    reader.onabort = () => reject(new Error("read-aborted"));
    reader.readAsDataURL(file);
  });
}
