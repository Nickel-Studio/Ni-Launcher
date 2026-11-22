import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
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

const STORAGE_KEY = "nickel.settings";

const THEME_MODES: readonly ThemeMode[] = ["system", "light", "dark"];
const ACCENTS: readonly Accent[] = ACCENT_IDS;
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

export type BackgroundKind = "image" | "video";

export function backgroundKind(path: string | null): BackgroundKind {
  const value = path?.trim() ?? "";
  return /^data:video\//i.test(value) || /^nickel-video:/i.test(value) ||
    /\.(mp4|m4v|webm|ogv)(?:[?#].*)?$/i.test(value)
    ? "video"
    : "image";
}

export function resolveBackgroundUrl(path: string | null): string | null {
  if (!path) return null;
  const value = path.trim();
  if (!value) return null;
  if (/^(https?:|data:|blob:|nickel-image:|nickel-video:|tauri:|\/\/)/i.test(value))
    return value;
  if (
    /^[a-zA-Z]:[\\/]/.test(value) ||
    value.startsWith("/") ||
    value.startsWith("\\\\")
  ) {
    try {
      if (backgroundKind(value) === "video")
        return convertFileSrc(value, "nickel-video");
      return convertFileSrc(value, "nickel-image");
    } catch {
      return null;
    }
  }
  return /^(asset:|file:|javascript:)/i.test(value) ? null : value;
}

/* ----------------------------------------------------------------------------
   Validated background protocols (image dataURL fallback only)
   -------------------------------------------------------------------------- */

export interface BackgroundSource {
  src: string | null;
  /**
   * Called from the rendered `<img onError>`. Returns true while a one-time data URL
   * fallback is in flight; `onFailed` runs if that fallback is unavailable too.
   */
  recover: (onFailed?: () => void) => boolean;
}

/**
 * Local images and videos use separate validated protocols; videos never become base64 IPC.
 * The image protocol is the regular path, so there is no pre-flight probe that would read every
 * file twice; `image_data_url` is requested only after the real element fails to load.
 */
export function useBackgroundSrc(path: string | null): BackgroundSource {
  const direct = resolveBackgroundUrl(path);
  const [fallback, setFallback] = useState<{ path: string; src: string | null } | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const recover = useCallback(
    (onFailed?: () => void) => {
      if (!path || !direct || !isTauri() || backgroundKind(path) === "video") return false;
      // One attempt per path: a failing data URL must not loop back into another request.
      if (fallback?.path === path) return false;
      setFallback({ path, src: null });
      invoke<string>("image_data_url", { path })
        .then((src) => {
          if (!alive.current) return;
          if (!src) throw new Error("empty-image");
          setFallback((current) => (current?.path === path ? { path, src } : current));
        })
        .catch(() => {
          if (alive.current) onFailed?.();
        });
      return true;
    },
    [path, direct, fallback]
  );

  const src = fallback?.path === path && fallback.src ? fallback.src : direct;
  return { src, recover };
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
  for (const key of [
    "versionListSource",
    "fileDownloadSource",
    "communityApiSource",
  ] as const) {
    out[key] = SOURCE_POLICIES.includes(raw[key] as SourcePolicy)
      ? (raw[key] as SourcePolicy)
      : DEFAULT_SETTINGS[key];
  }
  // 镜像基址由服务常量提供，不接受配置文件覆盖。
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
  useLayoutEffect(
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
