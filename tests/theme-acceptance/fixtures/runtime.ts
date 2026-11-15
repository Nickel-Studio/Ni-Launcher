import { DEFAULT_CUSTOM_THEME, startThemeRuntime, type ThemeRuntimeOptions } from "../../../src/lib/theme";

const root = document.getElementById("runtime-root")!;
const setTimeoutOriginal = window.setTimeout.bind(window);
const clearTimeoutOriginal = window.clearTimeout.bind(window);
const active = new Set<number>();
let created = 0;
let fired = 0;
let maximum = 0;
let writes = 0;
let cleanup: (() => void) | undefined;
const observer = new MutationObserver((records) => {
  writes += records.filter((record) => record.attributeName === "style").length;
});
observer.observe(root, { attributes: true });

window.setTimeout = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
  if (delay !== 500 || typeof callback !== "function")
    return setTimeoutOriginal(callback, delay, ...args);
  created += 1;
  const id = setTimeoutOriginal(() => {
    active.delete(id);
    fired += 1;
    callback(...args);
  }, delay);
  active.add(id);
  maximum = Math.max(maximum, active.size);
  return id;
}) as typeof window.setTimeout;
window.clearTimeout = ((id?: number) => {
  if (id !== undefined) active.delete(id);
  return clearTimeoutOriginal(id);
}) as typeof window.clearTimeout;

const api = {
  root,
  start(options: Partial<ThemeRuntimeOptions> = {}) {
    cleanup?.();
    cleanup = startThemeRuntime(root, {
      accent: "fortyTwo",
      themeMode: "dark",
      customTheme: { ...DEFAULT_CUSTOM_THEME },
      titlebarTint: false,
      ...options,
    });
  },
  stop() { cleanup?.(); cleanup = undefined; },
  snapshot() {
    return {
      active: active.size,
      created,
      fired,
      maximum,
      writes,
      accent: root.style.getPropertyValue("--accent"),
      dataset: { ...root.dataset },
      properties: Array.from(root.style).map((key) => [key, root.style.getPropertyValue(key)]),
    };
  },
  hidden(value: boolean) {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => value });
    document.dispatchEvent(new Event("visibilitychange"));
  },
  restoreVisibility() {
    Reflect.deleteProperty(document, "hidden");
    document.dispatchEvent(new Event("visibilitychange"));
  },
};
Object.assign(window, { __themeRuntime: api });
document.documentElement.dataset.fixtureReady = "true";
