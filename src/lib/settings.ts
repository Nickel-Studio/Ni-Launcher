import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import {
  ACCENT_IDS,
  DEFAULT_CUSTOM_THEME,
  normalizeCustomTheme,
  startThemeRuntime,
  type Accent,
  type CustomTheme,
} from "./theme";
export type { Accent, CustomTheme, ThemeDecoration } from "./theme";

/* ============================================================================
   Public contract
   ========================================================================== */

/**
 * 明暗模式（跟随系统 / 亮色 / 暗色）。
 * 注意：明暗模式 **不是「主题」** —— 项目说的「主题」指下面的 Accent（颜色主题）。
 */
export type ThemeMode = "system" | "light" | "dark";
export type DownloadSource = "official" | "bmclapi";
export type SourcePolicy = "mirror_first" | "official_auto" | "official_first";
export const SOURCE_POLICIES: readonly SourcePolicy[] = [
  "mirror_first",
  "official_auto",
  "official_first",
];
export const DEFAULT_DOWNLOAD_THREADS = 10;
export const MAX_DOWNLOAD_THREADS = 64;
export const MIB = 1024 * 1024;
export const MAX_DOWNLOAD_LIMIT_BPS = 128 * MIB;
export const COMMUNITY_MIRRORS = {
  modrinthApiMirror: "https://mod.mcimirror.top/modrinth/v2/",
  curseforgeApiMirror: "https://mod.mcimirror.top/curseforge/v1/",
  communityFileMirror: "https://mod.mcimirror.top/",
} as const;

export function normalizeMirrorBase(value: string): string | null {
  try {
    const url = new URL(value.trim());
    if (
      url.protocol !== "https:" ||
      !url.hostname ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    )
      return null;
    return `${url.href.replace(/\/+$/, "")}/`;
  } catch {
    return null;
  }
}

export interface AppSettings {
  themeMode: ThemeMode;
  accent: Accent;
  customTheme: CustomTheme;
  titlebarTint: boolean;
  backgroundImage: string | null;
  backgroundBlur: number;
  backgroundOpacity: number;
  privacyMode: boolean;
  /** 仅供旧调用者兼容；新入口分别使用三项策略。 */
  downloadSource: DownloadSource;
  versionListSource: SourcePolicy;
  fileDownloadSource: SourcePolicy;
  communityApiSource: SourcePolicy;
  modrinthApiMirror: string;
  curseforgeApiMirror: string;
  communityFileMirror: string;
  maxDownloadThreads: number;
  downloadLimitBps: number;
  verifyMicrosoftSsl: boolean;
  offlineUsername: string;
  accountType: "microsoft" | "littleskin" | "offline";
  littleskinServer: string;
  memoryMb: number;
  /** 前沿计划：添加孤立 `.minecraft` 时用上级目录名作为展示名。 */
  instanceNamingOptimize: boolean;
}

export const DEFAULT_SETTINGS: AppSettings = {
  themeMode: "dark",
  accent: "mono", // 颜色主题默认「黑与白」
  customTheme: { ...DEFAULT_CUSTOM_THEME },
  titlebarTint: false,
  backgroundImage: null,
  backgroundBlur: 0,
  backgroundOpacity: 0.5,
  privacyMode: false,
  downloadSource: "official",
  versionListSource: "official_auto",
  fileDownloadSource: "official_auto",
  communityApiSource: "official_auto",
  ...COMMUNITY_MIRRORS,
  maxDownloadThreads: DEFAULT_DOWNLOAD_THREADS,
  downloadLimitBps: 0,
  verifyMicrosoftSsl: true,
  offlineUsername: "Player",
  accountType: "microsoft",
  littleskinServer: "https://littleskin.cn",
  memoryMb: 4096,
  instanceNamingOptimize: false,
};

export interface SettingsContextValue {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
  ready: boolean;
  saveError: string;
}

/* ============================================================================
   Environment helpers
   ========================================================================== */

const STORAGE_KEY = "nowhere.settings";

const THEME_MODES: readonly ThemeMode[] = ["system", "light", "dark"];
const ACCENTS: readonly Accent[] = ACCENT_IDS;
const SOURCES: readonly DownloadSource[] = ["official", "bmclapi"];
const ACCOUNT_TYPES: readonly AppSettings["accountType"][] = [
  "microsoft",
  "littleskin",
  "offline",
];

export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof (window as unknown as { __TAURI_INTERNALS__?: unknown })
      .__TAURI_INTERNALS__ !== "undefined"
  );
}

export function resolveBackgroundUrl(path: string | null): string | null {
  if (!path) return null;
  const value = path.trim();
  if (!value) return null;
  if (/^(https?:|data:|blob:|asset:|tauri:|\/\/)/i.test(value)) return value;
  if (
    /^[a-zA-Z]:[\\/]/.test(value) ||
    value.startsWith("/") ||
    value.startsWith("\\\\")
  ) {
    try {
      return convertFileSrc(value);
    } catch {
      return value;
    }
  }
  return value;
}

/* ----------------------------------------------------------------------------
   Background source (asset protocol with dataURL fallback)
   -------------------------------------------------------------------------- */

/**
 * 背景图源。
 *
 * 先走 Tauri asset 协议（`convertFileSrc`）直接引用本地文件；
 * 若该 URL 加载失败（协议未启用 / 路径不在 scope 内 / CSP 限制），
 * 自动回退到 `image_data_url` 命令读取的 dataURL，保证背景图一定能渲染。
 */
export function useBackgroundSrc(path: string | null): string | null {
  const direct = resolveBackgroundUrl(path);
  const [src, setSrc] = useState<string | null>(direct);

  useEffect(() => {
    setSrc(resolveBackgroundUrl(path));
    if (!path || !direct || !isTauri()) return;
    let alive = true;
    const probe = new Image();
    probe.onerror = () => {
      invoke<string>("image_data_url", { path })
        .then((dataUrl) => {
          if (alive && dataUrl) setSrc(dataUrl);
        })
        .catch(() => undefined);
    };
    probe.src = direct;
    return () => {
      alive = false;
    };
  }, [path, direct]);

  return src;
}

/* ============================================================================
   Normalisation + persistence
   ========================================================================== */

function clampNumber(
  value: unknown,
  min: number,
  max: number,
  fallback: number
): number {
  const n = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

export function normalizeSettings(input: unknown): AppSettings {
  const raw = (input && typeof input === "object" ? input : {}) as Record<
    string,
    unknown
  >;
  const out: AppSettings = { ...DEFAULT_SETTINGS };
  if (THEME_MODES.includes(raw.themeMode as ThemeMode))
    out.themeMode = raw.themeMode as ThemeMode;
  if (ACCENTS.includes(raw.accent as Accent)) out.accent = raw.accent as Accent;
  out.customTheme = normalizeCustomTheme(raw.customTheme);
  out.titlebarTint =
    typeof raw.titlebarTint === "boolean" ? raw.titlebarTint : false;
  if (SOURCES.includes(raw.downloadSource as DownloadSource))
    out.downloadSource = raw.downloadSource as DownloadSource;
  const migrated: SourcePolicy =
    raw.downloadSource === "bmclapi"
      ? "mirror_first"
      : raw.downloadSource === "official"
      ? "official_first"
      : "official_auto";
  for (const key of [
    "versionListSource",
    "fileDownloadSource",
    "communityApiSource",
  ] as const) {
    out[key] = SOURCE_POLICIES.includes(raw[key] as SourcePolicy)
      ? (raw[key] as SourcePolicy)
      : key === "communityApiSource"
      ? "official_auto"
      : migrated;
  }
  // 镜像基址由应用固定；旧配置保留键形状，但不继续采用自定义地址。
  Object.assign(out, COMMUNITY_MIRRORS);
  out.maxDownloadThreads = Math.round(
    clampNumber(
      raw.maxDownloadThreads,
      1,
      MAX_DOWNLOAD_THREADS,
      DEFAULT_DOWNLOAD_THREADS
    )
  );
  out.downloadLimitBps = Math.round(
    clampNumber(raw.downloadLimitBps, 0, MAX_DOWNLOAD_LIMIT_BPS, 0)
  );
  out.verifyMicrosoftSsl =
    typeof raw.verifyMicrosoftSsl === "boolean" ? raw.verifyMicrosoftSsl : true;
  if (ACCOUNT_TYPES.includes(raw.accountType as AppSettings["accountType"]))
    out.accountType = raw.accountType as AppSettings["accountType"];
  out.backgroundImage =
    typeof raw.backgroundImage === "string" ? raw.backgroundImage : null;
  out.backgroundBlur = clampNumber(
    raw.backgroundBlur,
    0,
    40,
    DEFAULT_SETTINGS.backgroundBlur
  );
  out.backgroundOpacity = clampNumber(
    raw.backgroundOpacity,
    0,
    1,
    DEFAULT_SETTINGS.backgroundOpacity
  );
  out.privacyMode =
    typeof raw.privacyMode === "boolean"
      ? raw.privacyMode
      : DEFAULT_SETTINGS.privacyMode;
  out.offlineUsername =
    typeof raw.offlineUsername === "string"
      ? raw.offlineUsername.slice(0, 16)
      : DEFAULT_SETTINGS.offlineUsername;
  out.littleskinServer =
    typeof raw.littleskinServer === "string" && raw.littleskinServer.trim()
      ? raw.littleskinServer
      : DEFAULT_SETTINGS.littleskinServer;
  out.memoryMb = Math.round(
    clampNumber(raw.memoryMb, 512, 65536, DEFAULT_SETTINGS.memoryMb)
  );
  out.instanceNamingOptimize =
    typeof raw.instanceNamingOptimize === "boolean"
      ? raw.instanceNamingOptimize
      : DEFAULT_SETTINGS.instanceNamingOptimize;
  return out;
}

function readLocalSync(): AppSettings {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw) return normalizeSettings(JSON.parse(raw));
  } catch {
    // 损坏的首屏缓存不影响后端配置加载。
  }
  return normalizeSettings({});
}

async function loadSettings(): Promise<AppSettings> {
  if (isTauri()) {
    const json = await invoke<string>("settings_get");
    return normalizeSettings(JSON.parse(json || "{}"));
  }
  return readLocalSync();
}

let persistenceQueue: Promise<void> = Promise.resolve();
function persistSettings(settings: AppSettings): Promise<void> {
  const json = JSON.stringify(settings);
  const pending = persistenceQueue
    .catch(() => undefined)
    .then(async () => {
      if (isTauri()) await invoke("settings_set", { json });
      try {
        window.localStorage.setItem(STORAGE_KEY, json);
      } catch {
        /* 首屏缓存可选。 */
      }
    });
  persistenceQueue = pending;
  return pending;
}

/* ============================================================================
   Context + provider
   ========================================================================== */

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<AppSettings>(() => readLocalSync());
  const [ready, setReady] = useState(false);
  const [saveError, setSaveError] = useState("");

  useEffect(() => {
    let alive = true;
    void loadSettings()
      .then((loaded) => {
        if (alive) {
          setSettings(loaded);
          setReady(true);
        }
      })
      .catch(() => {
        if (alive) setSaveError("读取设置失败，请重新打开设置后重试");
      });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    if (!ready) return;
    let alive = true;
    void persistSettings(settings)
      .then(() => {
        if (alive) setSaveError("");
      })
      .catch(() => {
        if (alive)
          setSaveError("保存设置失败，请检查配置目录权限后重新调整设置");
      });
    return () => {
      alive = false;
    };
  }, [ready, settings]);

  const update = useCallback((patch: Partial<AppSettings>) => {
    setSettings((prev) => normalizeSettings({ ...prev, ...patch }));
  }, []);

  const {
    hue,
    saturation,
    lightness,
    titlebarHueShift,
    decoration,
    decorationOpacity,
  } = settings.customTheme;
  useEffect(
    () =>
      startThemeRuntime(document.documentElement, {
        accent: settings.accent,
        themeMode: settings.themeMode,
        titlebarTint: settings.titlebarTint,
        customTheme: {
          hue,
          saturation,
          lightness,
          titlebarHueShift,
          decoration,
          decorationOpacity,
        },
      }),
    [
      settings.accent,
      settings.themeMode,
      settings.titlebarTint,
      hue,
      saturation,
      lightness,
      titlebarHueShift,
      decoration,
      decorationOpacity,
    ]
  );

  useEffect(() => {
    document.documentElement.dataset.hasBg = settings.backgroundImage?.trim()
      ? "true"
      : "false";
  }, [settings.backgroundImage]);

  const value: SettingsContextValue = { settings, update, ready, saveError };
  return createElement(SettingsContext.Provider, { value }, children);
}

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx)
    throw new Error("useSettings() must be used inside <SettingsProvider>.");
  return ctx;
}
