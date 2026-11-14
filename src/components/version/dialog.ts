/**
 * 系统对话框：选择目录 / 二次确认。
 *
 * 这里直接 invoke dialog 插件的命令，而不是 import `@tauri-apps/plugin-dialog`：
 * 本仓库锁定前端依赖、不新增 npm 包，而该 npm 包做的事就是这几个 invoke。
 * 依据（已核对 tauri-plugin-dialog 2.8.4 源码）：
 * - 插件只注册了 `open` / `save` / `message` 三条命令，且
 *   `dialog:allow-open`、`dialog:allow-message` 已在 capabilities 中授权；
 * - `options` 结构体使用 `rename_all = "camelCase"`，故传 `directory` / `multiple` / `title`；
 * - `confirm`/`ask` 只是 message + OkCancel 的前端别名（权限亦为 allow-message 的别名）。
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";

/** 选择一个目录；用户取消时返回 `null`。 */
export async function pickDirectory(title: string): Promise<string | null> {
  if (!isTauri()) throw new Error("未连接到启动器后端，无法打开目录选择器");
  const selected = await invoke<string | string[] | null>("plugin:dialog|open", {
    options: { title, directory: true, multiple: false },
  });
  if (Array.isArray(selected)) {
    const first = selected[0];
    return typeof first === "string" && first.trim() ? first : null;
  }
  return typeof selected === "string" && selected.trim() ? selected : null;
}

/** 二次确认；用户取消或直接关闭窗口都视为「否」。 */
export async function confirmDialog(message: string, title: string): Promise<boolean> {
  if (!isTauri()) throw new Error("未连接到启动器后端，无法弹出确认框");
  const result = await invoke<string>("plugin:dialog|message", {
    title,
    message,
    kind: "warning",
    buttons: "OkCancel",
  });
  return result === "Ok";
}
