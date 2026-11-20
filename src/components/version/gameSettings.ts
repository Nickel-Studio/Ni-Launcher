import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";

/** 全局游戏设置只保存在 game.json；未知字段在读写时保持原样。 */
export interface GameSettings extends Record<string, unknown> {
  gameDir: string;
  javaPath: string;
  versionIsolation: boolean;
  windowTitle: string;
  customInfo: string;
  launcherVisibility: "keep" | "hide" | "hide-close" | "close" | "minimize";
  processPriority: "low" | "normal" | "high";
  windowMode:
    | "default"
    | "custom"
    | "fullscreen"
    | "same-as-launcher"
    | "maximize";
  windowWidth: number;
  windowHeight: number;
  memoryMode: "auto" | "custom";
  memoryMb: number;
  memoryManagement:
    | "auto-zgc"
    | "generational-zgc"
    | "g1"
    | "g1-tuned"
    | "custom"
    | "serial"
    | "parallel";
  highPerformanceGpu: boolean;
  disableWrapper: boolean;
  disableLwjglAgent: boolean;
  offlineSkin: "random" | "steve" | "alex" | "official" | "custom";
  offlineSkinName: string;
  offlineSkinPath: string;
  offlineSkinSlim: boolean;
  jvmArgs: string;
  gameArgs: string;
  preLaunchCommand: string;
}

export const DEFAULT_GAME_SETTINGS: GameSettings = {
  gameDir: "",
  javaPath: "",
  versionIsolation: true,
  windowTitle: "",
  customInfo: "",
  launcherVisibility: "keep",
  processPriority: "normal",
  windowMode: "default",
  windowWidth: 854,
  windowHeight: 480,
  memoryMode: "auto",
  memoryMb: 4096,
  memoryManagement: "g1-tuned",
  highPerformanceGpu: false,
  disableWrapper: false,
  disableLwjglAgent: false,
  offlineSkin: "random",
  offlineSkinName: "",
  offlineSkinPath: "",
  offlineSkinSlim: false,
  jvmArgs: "",
  gameArgs: "",
  preLaunchCommand: "",
};

const KEY = "nowhere.gameSettings";
const LEGACY_SETTINGS_KEY = "nowhere.settings";
export const GAME_MEMORY_MIN_MB = 512;
export const GAME_MEMORY_MAX_MB = 65536;

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function positiveInteger(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.max(1, Math.round(value))
    : fallback;
}

export function normalizeGameSettings(input: unknown): GameSettings {
  const raw = asRecord(input);
  return {
    ...raw,
    gameDir: typeof raw.gameDir === "string" ? raw.gameDir : "",
    javaPath: typeof raw.javaPath === "string" ? raw.javaPath : "",
    versionIsolation: raw.versionIsolation !== false,
    windowTitle: typeof raw.windowTitle === "string" ? raw.windowTitle : "",
    customInfo: typeof raw.customInfo === "string" ? raw.customInfo : "",
    launcherVisibility:
      raw.launcherVisibility === "hide" ||
      raw.launcherVisibility === "hide-close" ||
      raw.launcherVisibility === "close" ||
      raw.launcherVisibility === "minimize"
        ? raw.launcherVisibility
        : "keep",
    processPriority:
      raw.processPriority === "low" || raw.processPriority === "high"
        ? raw.processPriority
        : "normal",
    windowMode:
      raw.windowMode === "custom" ||
      raw.windowMode === "fullscreen" ||
      raw.windowMode === "same-as-launcher" ||
      raw.windowMode === "maximize"
        ? raw.windowMode
        : "default",
    windowWidth: positiveInteger(
      raw.windowWidth,
      DEFAULT_GAME_SETTINGS.windowWidth
    ),
    windowHeight: positiveInteger(
      raw.windowHeight,
      DEFAULT_GAME_SETTINGS.windowHeight
    ),
    memoryMode: raw.memoryMode === "custom" ? "custom" : "auto",
    // 保留已保存值，超物理内存时由后端给出可执行错误，不在前端静默截断。
    memoryMb: positiveInteger(raw.memoryMb, DEFAULT_GAME_SETTINGS.memoryMb),
    memoryManagement:
      raw.memoryManagement === "default"
        ? "custom"
        : raw.memoryManagement === "auto-zgc" ||
          raw.memoryManagement === "generational-zgc" ||
          raw.memoryManagement === "g1" ||
          raw.memoryManagement === "g1-tuned" ||
          raw.memoryManagement === "custom" ||
          raw.memoryManagement === "serial" ||
          raw.memoryManagement === "parallel"
        ? raw.memoryManagement
        : "g1-tuned",
    highPerformanceGpu: raw.highPerformanceGpu === true,
    disableWrapper: raw.disableWrapper === true,
    disableLwjglAgent: raw.disableLwjglAgent === true,
    offlineSkin:
      raw.offlineSkin === "steve" ||
      raw.offlineSkin === "alex" ||
      raw.offlineSkin === "official" ||
      raw.offlineSkin === "custom"
        ? raw.offlineSkin
        : "random",
    offlineSkinName:
      typeof raw.offlineSkinName === "string" ? raw.offlineSkinName : "",
    offlineSkinPath:
      typeof raw.offlineSkinPath === "string" ? raw.offlineSkinPath : "",
    offlineSkinSlim: raw.offlineSkinSlim === true,
    jvmArgs: typeof raw.jvmArgs === "string" ? raw.jvmArgs : "",
    gameArgs: typeof raw.gameArgs === "string" ? raw.gameArgs : "",
    preLaunchCommand:
      typeof raw.preLaunchCommand === "string" ? raw.preLaunchCommand : "",
  };
}

let persistenceQueue: Promise<void> = Promise.resolve();

/** 桌面读取失败直接抛出，避免以本地缓存或默认值覆盖已有配置。 */
export async function loadGameSettings(): Promise<GameSettings> {
  await persistenceQueue.catch(() => undefined);
  const desktop = isTauri();
  const json = desktop
    ? await invoke<string>("game_settings_get")
    : window.localStorage.getItem(KEY);
  const raw = asRecord(JSON.parse(json || "{}"));
  if (raw.memoryMb === undefined) {
    const legacyJson = desktop
      ? await invoke<string>("settings_get")
      : window.localStorage.getItem(LEGACY_SETTINGS_KEY);
    const legacy = asRecord(JSON.parse(legacyJson || "{}"));
    if (
      typeof legacy.memoryMb === "number" &&
      Number.isFinite(legacy.memoryMb)
    ) {
      raw.memoryMb = legacy.memoryMb;
      // 老版内存值表示手动分配；迁移时不能悄悄改成自动。
      if (raw.memoryMode === undefined) raw.memoryMode = "custom";
    }
  }
  return normalizeGameSettings(raw);
}

/** 串行写入；失败 reject，调用方保留编辑值并提供重试入口。 */
export async function saveGameSettings(
  settings: GameSettings
): Promise<{ persisted: boolean }> {
  const json = JSON.stringify(normalizeGameSettings(settings));
  const pending = persistenceQueue
    .catch(() => undefined)
    .then(async () => {
      if (isTauri()) {
        await invoke("game_settings_set", { json });
        try {
          window.localStorage.setItem(KEY, json);
        } catch {
          // 桌面缓存不影响已经成功的磁盘写入。
        }
      } else {
        window.localStorage.setItem(KEY, json);
      }
    });
  persistenceQueue = pending;
  await pending;
  return { persisted: true };
}
