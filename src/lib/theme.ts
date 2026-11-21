export const ACCENT_IDS = [
  "mono",
  "emerald",
  "blue",
  "violet",
  "amber",
  "rose",
  "nickel",
  "copper",
  "abyss",
  "paper",
  "fortyTwo",
  "custom",
] as const;

export type Accent = (typeof ACCENT_IDS)[number];
export type ResolvedThemeMode = "light" | "dark";
export const THEME_DECORATIONS = [
  "none",
  "lattice",
  "ribbons",
  "orbits",
  "folds",
] as const;
export type ThemeDecoration = (typeof THEME_DECORATIONS)[number];

export interface CustomTheme {
  hue: number;
  saturation: number;
  lightness: number;
  titlebarHueShift: number;
  decoration: ThemeDecoration;
  decorationOpacity: number;
}

export const CUSTOM_THEME_BOUNDS = {
  hue: [0, 360],
  saturation: [0, 100],
  lightness: [0, 100],
  titlebarHueShift: [-180, 180],
  decorationOpacity: [0, 1],
} as const;

export const DEFAULT_CUSTOM_THEME: Readonly<CustomTheme> = Object.freeze({
  hue: 174,
  saturation: 39,
  lightness: 44,
  titlebarHueShift: 0,
  decoration: "lattice",
  decorationOpacity: 0.35,
});

export const THEME_COLORS: Readonly<Record<Exclude<Accent, "custom">, string>> =
  {
    mono: "#f2f4f5",
    emerald: "#3ecf8e",
    blue: "#5a97e8",
    violet: "#9b8cf0",
    amber: "#e0aa4c",
    rose: "#ea6f8b",
    nickel: "#8dbcae",
    copper: "#e4a27c",
    abyss: "#75acd1",
    paper: "#e3c48d",
    fortyTwo: "#459c93",
  };

export const FORTY_TWO_CYCLE_MS = 180_000;
export const FORTY_TWO_END = "#bcdec4";
export const THEME_TICK_MS = 500;
export type Rgb = readonly [number, number, number];

export function normalizeCustomTheme(input: unknown): CustomTheme {
  const raw =
    input && typeof input === "object" && !Array.isArray(input)
      ? (input as Record<string, unknown>)
      : {};
  const result: CustomTheme = { ...DEFAULT_CUSTOM_THEME };
  for (const key of Object.keys(CUSTOM_THEME_BOUNDS) as Array<
    keyof typeof CUSTOM_THEME_BOUNDS
  >) {
    const value = raw[key];
    const [min, max] = CUSTOM_THEME_BOUNDS[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      result[key] = Math.min(max, Math.max(min, value));
    }
  }
  if (THEME_DECORATIONS.includes(raw.decoration as ThemeDecoration)) {
    result.decoration = raw.decoration as ThemeDecoration;
  }
  return result;
}

export function hexToRgb(hex: string): Rgb {
  const value = Number.parseInt(hex.slice(1), 16);
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
}

export function rgbToHex(rgb: Rgb): string {
  return `#${rgb
    .map((value) => Math.round(value).toString(16).padStart(2, "0"))
    .join("")}`;
}

export function hslToRgb(
  hue: number,
  saturation: number,
  lightness: number
): Rgb {
  const h = (((hue % 360) + 360) % 360) / 60;
  const s = saturation / 100;
  const l = lightness / 100;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs((h % 2) - 1));
  const m = l - c / 2;
  const channels: Rgb =
    h < 1
      ? [c, x, 0]
      : h < 2
      ? [x, c, 0]
      : h < 3
      ? [0, c, x]
      : h < 4
      ? [0, x, c]
      : h < 5
      ? [x, 0, c]
      : [c, 0, x];
  return channels.map((value) =>
    Math.round((value + m) * 255)
  ) as unknown as Rgb;
}

function rgbToHsl(rgb: Rgb): Rgb {
  const [r, g, b] = rgb.map((value) => value / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  const l = (max + min) / 2;
  let h = 0;
  if (delta) {
    h =
      max === r
        ? ((g - b) / delta) % 6
        : max === g
        ? (b - r) / delta + 2
        : (r - g) / delta + 4;
  }
  return [
    (h * 60 + 360) % 360,
    delta ? (delta / (1 - Math.abs(2 * l - 1))) * 100 : 0,
    l * 100,
  ];
}

function mixRgb(a: Rgb, b: Rgb, amount: number): Rgb {
  return a.map((value, i) =>
    Math.round(value + (b[i] - value) * amount)
  ) as unknown as Rgb;
}

export function relativeLuminance(rgb: Rgb): number {
  const channels = rgb.map((value) => {
    const channel = value / 255;
    return channel <= 0.04045
      ? channel / 12.92
      : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

export function contrastRatio(a: Rgb, b: Rgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

export function fortyTwoColor(elapsedMs: number): Rgb {
  const phase =
    ((elapsedMs % FORTY_TWO_CYCLE_MS) + FORTY_TWO_CYCLE_MS) %
    FORTY_TWO_CYCLE_MS;
  const amount = (1 - Math.cos((phase / FORTY_TWO_CYCLE_MS) * Math.PI * 2)) / 2;
  return mixRgb(
    hexToRgb(THEME_COLORS.fortyTwo),
    hexToRgb(FORTY_TWO_END),
    amount
  );
}

export function accentColor(
  accent: Accent,
  customTheme: CustomTheme,
  mode: ResolvedThemeMode,
  elapsedMs = 0
): Rgb {
  if (accent === "custom")
    return hslToRgb(
      customTheme.hue,
      customTheme.saturation,
      customTheme.lightness
    );
  if (accent === "fortyTwo") return fortyTwoColor(elapsedMs);
  return hexToRgb(
    accent === "mono" && mode === "light" ? "#14171a" : THEME_COLORS[accent]
  );
}

export function createAccentTokens(
  color: Rgb,
  mode: ResolvedThemeMode,
  titlebarHueShift = 0
): Record<string, string> {
  const black: Rgb = [0, 0, 0];
  const white: Rgb = [255, 255, 255];
  const foreground =
    contrastRatio(color, black) >= contrastRatio(color, white) ? black : white;
  const hover = mixRgb(color, foreground === black ? white : black, 0.1);
  // Use the least forgiving neutral surface so text survives the entire fill ladder.
  const surface = hexToRgb(mode === "light" ? "#dfe1e5" : "#353a3c");
  const text = mode === "light" ? hexToRgb("#14171a") : hexToRgb("#f2f4f5");
  let signal = color;
  for (
    let step = 1;
    contrastRatio(signal, surface) < 4.8 && step <= 20;
    step += 1
  ) {
    signal = mixRgb(color, text, step / 20);
  }
  const [h, s, l] = rgbToHsl(color);
  const tint = titlebarHueShift ? hslToRgb(h + titlebarHueShift, s, l) : color;
  const chromeBase = hexToRgb(mode === "light" ? "#fcfdfe" : "#0e1011");
  const chrome = mixRgb(chromeBase, tint, mode === "light" ? 0.12 : 0.14);
  return {
    "--accent": rgbToHex(color),
    "--accent-strong": rgbToHex(hover),
    "--accent-fg": rgbToHex(foreground),
    "--accent-soft": `rgba(${color.join(", ")}, 0.14)`,
    "--accent-ring": `rgba(${signal.join(", ")}, 0.4)`,
    "--accent-text": rgbToHex(signal),
    "--theme-ink": rgbToHex(color),
    "--theme-chrome-bg": `rgba(${chrome.join(", ")}, ${
      mode === "light" ? 0.9 : 0.88
    })`,
    "--theme-chrome-text": rgbToHex(text),
  };
}

export interface ThemeRuntimeOptions {
  accent: Accent;
  themeMode: "system" | ResolvedThemeMode;
  customTheme: CustomTheme;
  titlebarTint: boolean;
}

const RUNTIME_PROPERTIES = [
  "--accent",
  "--accent-strong",
  "--accent-fg",
  "--accent-soft",
  "--accent-ring",
  "--accent-text",
  "--theme-ink",
  "--theme-chrome-bg",
  "--theme-chrome-text",
  "--theme-decoration-opacity",
] as const;
const CSS_ACCENTS: readonly Accent[] = ACCENT_IDS.slice(0, 6);

// The clock paints CSS only; hiding the document pauses it without notifying React.
export function startThemeRuntime(
  root: HTMLElement,
  options: ThemeRuntimeOptions
): () => void {
  const doc = root.ownerDocument;
  const win = doc.defaultView;
  if (!win) return () => {};
  const scheme = win.matchMedia("(prefers-color-scheme: dark)");
  const motion = win.matchMedia("(prefers-reduced-motion: reduce)");
  const previous = new Map(
    RUNTIME_PROPERTIES.map((key) => [
      key,
      [root.style.getPropertyValue(key), root.style.getPropertyPriority(key)],
    ])
  );
  const datasetKeys = [
    "accent",
    "theme",
    "titlebarTint",
    "themeMotion",
    "richTheme",
  ] as const;
  const previousDataset = new Map(
    datasetKeys.map((key) => [key, root.dataset[key]])
  );
  const written = new Map<string, string>();
  const custom = normalizeCustomTheme(options.customTheme);
  let timer: number | undefined;
  let elapsed = 0;
  let activeSince: number | null = null;
  let disposed = false;

  root.dataset.accent = options.accent;
  root.dataset.titlebarTint = String(options.titlebarTint);
  root.dataset.richTheme = String(!CSS_ACCENTS.includes(options.accent));

  const paint = () => {
    const mode: ResolvedThemeMode =
      options.themeMode === "system"
        ? scheme.matches
          ? "dark"
          : "light"
        : options.themeMode;
    root.dataset.theme = mode;
    const current =
      elapsed +
      (activeSince === null ? 0 : win.performance.now() - activeSince);
    const color = accentColor(
      options.accent,
      custom,
      mode,
      motion.matches ? 0 : current
    );
    const tokens = createAccentTokens(
      color,
      mode,
      options.accent === "custom" && options.titlebarTint
        ? custom.titlebarHueShift
        : 0
    );
    tokens["--theme-decoration-opacity"] = String(
      options.accent === "custom" ? custom.decorationOpacity : 0.35
    );
    for (const [key, value] of Object.entries(tokens)) {
      if (
        CSS_ACCENTS.includes(options.accent) &&
        key.startsWith("--accent") &&
        key !== "--accent-text"
      )
        continue;
      if (written.get(key) !== value) {
        root.style.setProperty(key, value);
        written.set(key, value);
      }
    }
  };

  const pause = () => {
    if (timer !== undefined) win.clearTimeout(timer);
    timer = undefined;
    if (activeSince !== null) elapsed += win.performance.now() - activeSince;
    activeSince = null;
  };
  const tick = () => {
    if (disposed) return;
    paint();
    timer = win.setTimeout(tick, THEME_TICK_MS);
  };
  const sync = () => {
    if (disposed) return;
    pause();
    const running =
      options.accent === "fortyTwo" && !motion.matches && !doc.hidden;
    root.dataset.themeMotion = doc.hidden
      ? "paused"
      : motion.matches
      ? "reduced"
      : running
      ? "running"
      : "static";
    if (running) activeSince = win.performance.now();
    paint();
    if (running) timer = win.setTimeout(tick, THEME_TICK_MS);
  };

  scheme.addEventListener("change", sync);
  motion.addEventListener("change", sync);
  doc.addEventListener("visibilitychange", sync);
  sync();

  return () => {
    disposed = true;
    pause();
    scheme.removeEventListener("change", sync);
    motion.removeEventListener("change", sync);
    doc.removeEventListener("visibilitychange", sync);
    for (const key of written.keys()) {
      const [value, priority] = previous.get(
        key as (typeof RUNTIME_PROPERTIES)[number]
      ) ?? ["", ""];
      if (value) root.style.setProperty(key, value, priority);
      else root.style.removeProperty(key);
    }
    for (const key of datasetKeys) {
      const value = previousDataset.get(key);
      if (value === undefined) delete root.dataset[key];
      else root.dataset[key] = value;
    }
  };
}
