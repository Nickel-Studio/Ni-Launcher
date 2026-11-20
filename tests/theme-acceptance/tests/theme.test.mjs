import assert from "node:assert/strict";
import test, { after } from "node:test";
import { createServer } from "vite";

const server = await createServer({
  configFile: false,
  root: process.cwd(),
  appType: "custom",
  logLevel: "silent",
  server: { middlewareMode: true, hmr: false },
});
after(() => server.close());
const theme = await server.ssrLoadModule("/src/lib/theme.ts");
const settings = await server.ssrLoadModule("/src/lib/settings.ts");
const { ACCENT_OPTIONS } = await server.ssrLoadModule("/src/components/settings/accents.ts");
const expectedOrder = ["mono", "emerald", "blue", "violet", "amber", "rose", "nickel", "copper", "abyss", "paper", "fortyTwo", "custom"];
const fields = ["hue", "saturation", "lightness", "titlebarHueShift", "decorationOpacity"];

class Target {
  listeners = new Map();
  addEventListener(name, listener) {
    const set = this.listeners.get(name) ?? new Set();
    set.add(listener);
    this.listeners.set(name, set);
  }
  removeEventListener(name, listener) { this.listeners.get(name)?.delete(listener); }
  dispatch(name) { for (const listener of this.listeners.get(name) ?? []) listener(); }
  count() { return [...this.listeners.values()].reduce((sum, set) => sum + set.size, 0); }
}

function environment() {
  const scheme = Object.assign(new Target(), { matches: false });
  const motion = Object.assign(new Target(), { matches: false });
  const doc = Object.assign(new Target(), { hidden: false });
  const values = new Map();
  const jobs = new Map();
  let now = 0;
  let id = 0;
  let fired = 0;
  let maximum = 0;
  let writes = 0;
  const style = {
    getPropertyValue: (key) => values.get(key)?.[0] ?? "",
    getPropertyPriority: (key) => values.get(key)?.[1] ?? "",
    setProperty(key, value, priority = "") { writes += 1; values.set(key, [value, priority]); },
    removeProperty(key) { values.delete(key); },
  };
  const win = {
    performance: { now: () => now },
    matchMedia: (query) => query.includes("reduced-motion") ? motion : scheme,
    setTimeout(callback, delay) {
      assert.equal(delay, theme.THEME_TICK_MS);
      jobs.set(++id, { callback, due: now + delay });
      maximum = Math.max(maximum, jobs.size);
      return id;
    },
    clearTimeout: (key) => jobs.delete(key),
  };
  doc.defaultView = win;
  const root = { ownerDocument: doc, style, dataset: {} };
  return {
    root, doc, scheme, motion, values, jobs,
    get maximum() { return maximum; },
    get fired() { return fired; },
    get writes() { return writes; },
    advance(ms) {
      const target = now + ms;
      let next;
      while ((next = [...jobs.entries()].sort((a, b) => a[1].due - b[1].due)[0]) && next[1].due <= target) {
        now = next[1].due;
        jobs.delete(next[0]);
        fired += 1;
        next[1].callback();
      }
      now = target;
    },
    start(options = {}) {
      return theme.startThemeRuntime(root, {
        accent: "fortyTwo", themeMode: "dark", customTheme: { ...theme.DEFAULT_CUSTOM_THEME }, titlebarTint: false, ...options,
      });
    },
  };
}

function actualContrast(a, b) { return theme.contrastRatio(theme.hexToRgb(a), theme.hexToRgb(b)); }

function composite(color, background) {
  const parts = color.match(/[\d.]+/g).map(Number);
  return parts.slice(0, 3).map((value, index) => value * parts[3] + background[index] * (1 - parts[3]));
}

test("palette, labels and persisted whitelist share the exact twelve-entry order", () => {
  assert.deepEqual([...theme.ACCENT_IDS], expectedOrder);
  assert.deepEqual(ACCENT_OPTIONS.map((option) => option.value), expectedOrder);
  assert.deepEqual(ACCENT_OPTIONS.map((option) => option.label), ["黑与白", "翡翠绿", "海蓝色", "魅紫色", "琥珀色", "玫瑰红", "镍光", "铜霞", "深海", "纸灯", "四十二", "自定义"]);
  for (const accent of expectedOrder) assert.equal(settings.normalizeSettings({ accent }).accent, accent);
  assert.equal(settings.normalizeSettings({ accent: "locked" }).accent, "mono");
  assert.equal(settings.normalizeSettings({ accent: "fortyTwo" }).accent, "fortyTwo");
});

test("CSS accents retain stylesheet ownership, including light monochrome inversion", () => {
  assert.deepEqual(expectedOrder.slice(0, 6).map((accent) => theme.THEME_COLORS[accent]), ["#f2f4f5", "#3ecf8e", "#5a97e8", "#9b8cf0", "#e0aa4c", "#ea6f8b"]);
  assert.equal(theme.rgbToHex(theme.accentColor("mono", theme.DEFAULT_CUSTOM_THEME, "light")), "#14171a");
  for (const accent of expectedOrder.slice(0, 6)) {
    const env = environment();
    env.root.style.setProperty("--accent", "#123456", "important");
    const stop = env.start({ accent });
    assert.equal(env.root.style.getPropertyValue("--accent"), "#123456");
    assert.equal(env.root.style.getPropertyPriority("--accent"), "important");
    assert.equal(env.root.dataset.richTheme, "false");
    assert.equal(env.jobs.size, 0);
    stop();
    assert.equal(env.root.style.getPropertyValue("--accent"), "#123456");
  }
});

test("custom theme clamps every finite bound and preserves exact boundary values", () => {
  for (const field of fields) {
    const [min, max] = theme.CUSTOM_THEME_BOUNDS[field];
    assert.equal(theme.normalizeCustomTheme({ [field]: min - 0.01 })[field], min);
    assert.equal(theme.normalizeCustomTheme({ [field]: max + 0.01 })[field], max);
    for (const value of [min, max, (min + max) / 2]) assert.equal(theme.normalizeCustomTheme({ [field]: value })[field], value);
  }
  for (const decoration of theme.THEME_DECORATIONS) assert.equal(theme.normalizeCustomTheme({ decoration }).decoration, decoration);
  assert.deepEqual(theme.normalizeCustomTheme({ hue: 999, saturation: -9, lightness: 999, titlebarHueShift: -999, decoration: "invalid", decorationOpacity: 9 }), { hue: 360, saturation: 0, lightness: 100, titlebarHueShift: -180, decoration: "lattice", decorationOpacity: 1 });
});

test("invalid custom input cannot coerce private, missing or non-finite values into settings", () => {
  for (const input of [null, undefined, [], false, "42", 42]) assert.deepEqual(theme.normalizeCustomTheme(input), theme.DEFAULT_CUSTOM_THEME);
  for (const field of fields) {
    for (const value of [NaN, Infinity, -Infinity, "42", null, true, [], {}]) assert.equal(theme.normalizeCustomTheme({ [field]: value })[field], theme.DEFAULT_CUSTOM_THEME[field]);
  }
  const normalized = theme.normalizeCustomTheme({ ...theme.DEFAULT_CUSTOM_THEME, accessToken: "private", eulaAccepted: true, updateHistory: ["private"] });
  assert.deepEqual(normalized, theme.DEFAULT_CUSTOM_THEME);
  assert.equal(Object.keys(normalized).length, 6);
  assert.ok(Object.isFrozen(theme.DEFAULT_CUSTOM_THEME));
});

test("settings keeps new fields optional, strictly boolean and independently normalized", () => {
  assert.equal(settings.DEFAULT_SETTINGS.titlebarTint, false);
  for (const value of [undefined, null, "true", 1, [], {}]) assert.equal(settings.normalizeSettings({ titlebarTint: value }).titlebarTint, false);
  assert.equal(settings.normalizeSettings({ titlebarTint: true }).titlebarTint, true);
  const a = settings.normalizeSettings({});
  const b = settings.normalizeSettings({});
  a.customTheme.hue = 20;
  assert.equal(b.customTheme.hue, theme.DEFAULT_CUSTOM_THEME.hue);
  assert.deepEqual(settings.normalizeSettings(JSON.parse(JSON.stringify({ accent: "custom", titlebarTint: true, customTheme: theme.DEFAULT_CUSTOM_THEME }))).customTheme, theme.DEFAULT_CUSTOM_THEME);
});

test("forty-two reaches both exact endpoints and returns in 180000 milliseconds", () => {
  assert.equal(theme.FORTY_TWO_CYCLE_MS, 180000);
  assert.equal(theme.THEME_TICK_MS, 500);
  for (const time of [0, 180000, 360000, -180000]) assert.equal(theme.rgbToHex(theme.fortyTwoColor(time)), "#459c93");
  for (const time of [90000, 270000, -90000]) assert.equal(theme.rgbToHex(theme.fortyTwoColor(time)), "#bcdec4");
  assert.equal(theme.rgbToHex(theme.fortyTwoColor(45000)), "#81bdac");
  assert.deepEqual(theme.fortyTwoColor(45000), theme.fortyTwoColor(135000));
  let prior = theme.fortyTwoColor(0);
  for (let time = 500; time <= 90000; time += 500) {
    const current = theme.fortyTwoColor(time);
    assert.ok(current.every((value, index) => value >= prior[index]));
    assert.ok(current.every((value, index) => value - prior[index] <= 2));
    prior = current;
  }
});

test("all new tokens keep primary, hover, signal text and tinted chrome readable at color extremes", () => {
  for (const mode of ["light", "dark"]) {
    const surface = theme.hexToRgb(mode === "light" ? "#dfe1e5" : "#353a3c");
    const colors = expectedOrder.slice(6, 11).flatMap((accent) => [0, 45000, 90000, 135000].map((time) => theme.accentColor(accent, theme.DEFAULT_CUSTOM_THEME, mode, time)));
    for (let hue = 0; hue <= 360; hue += 15) {
      for (const saturation of [0, 39, 100]) {
        for (const lightness of [0, 1, 20, 44, 50, 80, 99, 100]) colors.push(theme.hslToRgb(hue, saturation, lightness));
      }
    }
    for (const color of colors) {
      for (const shift of [-180, 0, 180]) {
        const tokens = theme.createAccentTokens(color, mode, shift);
        assert.ok(actualContrast(tokens["--accent"], tokens["--accent-fg"]) >= 4.5, JSON.stringify({ mode, color, tokens }));
        assert.ok(actualContrast(tokens["--accent-strong"], tokens["--accent-fg"]) >= 4.5);
        assert.ok(theme.contrastRatio(theme.hexToRgb(tokens["--accent-text"]), surface) >= 4.5);
        for (const backdrop of [[0, 0, 0], [255, 255, 255]]) {
          assert.ok(theme.contrastRatio(theme.hexToRgb(tokens["--theme-chrome-text"]), composite(tokens["--theme-chrome-bg"], backdrop)) >= 4.5);
        }
      }
    }
  }
});

test("titlebar hue shift changes chrome only and never the primary palette", () => {
  const color = theme.hslToRgb(210, 90, 50);
  for (const mode of ["light", "dark"]) {
    const baseline = theme.createAccentTokens(color, mode);
    const shifted = theme.createAccentTokens(color, mode, 90);
    assert.notEqual(baseline["--theme-chrome-bg"], shifted["--theme-chrome-bg"]);
    for (const key of ["--accent", "--accent-strong", "--accent-fg", "--accent-text", "--theme-ink"]) assert.equal(baseline[key], shifted[key]);
  }
});

test("runtime executes one 500ms CSS timer, 360 ticks per full cycle, and restores all owned state", () => {
  const env = environment();
  env.root.style.setProperty("--accent", "#123456", "important");
  env.root.style.setProperty("--unrelated", "retained");
  env.root.dataset.accent = "before";
  env.root.dataset.unrelated = "retained";
  const stop = env.start();
  assert.equal(env.root.style.getPropertyValue("--accent"), "#459c93");
  assert.equal(env.jobs.size, 1);
  env.advance(90000);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#bcdec4");
  env.advance(90000);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#459c93");
  assert.equal(env.fired, 360);
  assert.equal(env.maximum, 1);
  const before = env.writes;
  env.scheme.dispatch("change");
  env.scheme.dispatch("change");
  assert.equal(env.jobs.size, 1);
  assert.equal(env.writes, before);
  stop();
  stop();
  assert.equal(env.jobs.size, 0);
  assert.equal(env.doc.count() + env.scheme.count() + env.motion.count(), 0);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#123456");
  assert.equal(env.root.style.getPropertyPriority("--accent"), "important");
  assert.equal(env.root.style.getPropertyValue("--unrelated"), "retained");
  assert.deepEqual(env.root.dataset, { accent: "before", unrelated: "retained" });
  env.advance(5000);
  assert.equal(env.fired, 360);
});

test("hidden documents pause elapsed active time and resume without duplicate jobs", () => {
  const env = environment();
  const stop = env.start();
  env.advance(45000);
  const before = env.root.style.getPropertyValue("--accent");
  env.doc.hidden = true;
  env.doc.dispatch("visibilitychange");
  assert.equal(env.root.dataset.themeMotion, "paused");
  assert.equal(env.jobs.size, 0);
  env.advance(180000);
  assert.equal(env.root.style.getPropertyValue("--accent"), before);
  env.doc.hidden = false;
  env.doc.dispatch("visibilitychange");
  env.doc.dispatch("visibilitychange");
  assert.equal(env.jobs.size, 1);
  env.advance(45000);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#bcdec4");
  assert.equal(env.maximum, 1);
  stop();
});

test("reduced motion freezes the start endpoint and system scheme changes paint immediately", () => {
  const env = environment();
  env.motion.matches = true;
  const stop = env.start({ themeMode: "system" });
  assert.equal(env.root.dataset.theme, "light");
  assert.equal(env.root.dataset.themeMotion, "reduced");
  assert.equal(env.jobs.size, 0);
  env.advance(180000);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#459c93");
  env.scheme.matches = true;
  env.scheme.dispatch("change");
  assert.equal(env.root.dataset.theme, "dark");
  env.motion.matches = false;
  env.motion.dispatch("change");
  assert.equal(env.jobs.size, 1);
  env.advance(45000);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#81bdac");
  env.motion.matches = true;
  env.motion.dispatch("change");
  assert.equal(env.jobs.size, 0);
  assert.equal(env.root.style.getPropertyValue("--accent"), "#459c93");
  stop();
});

test("static themes never schedule a job and repeated mount-cleanup leaves no listeners", () => {
  const env = environment();
  for (let pass = 0; pass < 3; pass += 1) {
    for (const accent of expectedOrder) {
      const stop = env.start({ accent, titlebarTint: true });
      assert.equal(env.jobs.size, accent === "fortyTwo" ? 1 : 0);
      assert.equal(env.root.dataset.richTheme, String(expectedOrder.indexOf(accent) >= 6));
      stop();
      assert.equal(env.jobs.size, 0);
      assert.equal(env.doc.count() + env.scheme.count() + env.motion.count(), 0);
    }
  }
});
