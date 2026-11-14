import { createRoot } from "react-dom/client";
import More from "../../src/pages/More";
import { TitleBar } from "../../src/components/TitleBar";
import { SettingsProvider, DEFAULT_SETTINGS } from "../../src/lib/settings";
import { setRoute } from "../../src/lib/nav";
import { PROJECT_URL, CODE_LICENSE_URL, ASSETS_LICENSE_URL } from "../../src/lib/brand";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/styles/shell.css";

interface Check { name: string; ok: boolean; }
const checks: Check[] = [];
const opened: string[] = [];
let failOpen = false;

const host = window as unknown as {
  __TAURI_INTERNALS__: {
    invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
    convertFileSrc: (path: string) => string;
  };
  __brandRegression: { checks: Check[]; opened: string[]; setFailOpen: (value: boolean) => void };
};
host.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    if (command === "settings_get") return JSON.stringify(DEFAULT_SETTINGS);
    if (command === "settings_set") return null;
    if (command === "plugin:opener|open_url") {
      if (failOpen) throw new Error("open failed");
      opened.push(String(args.url));
      return null;
    }
    return null;
  },
  convertFileSrc: (path) => path,
};
host.__brandRegression = { checks, opened, setFailOpen: (value) => { failOpen = value; } };
localStorage.setItem("nowhere.settings", JSON.stringify(DEFAULT_SETTINGS));
setRoute("more");
createRoot(document.getElementById("root")!).render(
  <SettingsProvider>
    <main className="brand-fixture">
      <TitleBar />
      <div className="brand-fixture__page"><More /></div>
      <p id="brand-result" aria-live="polite">正在运行品牌回归</p>
    </main>
  </SettingsProvider>
);

function check(name: string, ok: boolean): void {
  checks.push({ name, ok });
  if (!ok) throw new Error(name);
}
async function waitFor<T>(read: () => T | null | undefined | false): Promise<T> {
  const end = Date.now() + 5000;
  while (Date.now() < end) {
    const value = read();
    if (value) return value as T;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error("等待品牌界面超时");
}
async function run(): Promise<void> {
  const about = await waitFor(() => [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === "关于"));
  about.click();
  const page = await waitFor(() => document.querySelector<HTMLElement>(".mo-about"));
  check("完整名称", page.querySelector(".mo-about__name")?.textContent === "Nickel");
  check("标题栏名称", document.querySelector(".nw-brand__text")?.textContent === "Nickel");
  check("关于页无技术栈", !/Tauri|React|TypeScript|Rust/.test(page.textContent ?? ""));
  check("开发者与鸣谢", ["ItzArona", "ItzPlana", "龙猫腾跃", "PCL 启动器", "PCL-CE", "LinuxDO", "ShittimChests"].every((name) => page.textContent?.includes(name)));
  check("文字鸣谢不含按钮", !page.querySelector('[aria-label="龙猫腾跃"] button'));
  const images = [...page.querySelectorAll<HTMLImageElement>("img")];
  await waitFor(() => images.every((image) => image.complete && image.naturalWidth > 0));
  check("本地徽标与头像", images.length === 7 && images.every((image) => new URL(image.src).origin === location.origin));
  for (const url of [PROJECT_URL, CODE_LICENSE_URL, ASSETS_LICENSE_URL]) {
    page.querySelector<HTMLButtonElement>(`[data-brand-url="${url}"]`)!.click();
    await waitFor(() => opened.includes(url));
  }
  check("项目与许可外链", opened.length === 3);
  failOpen = true;
  page.querySelector<HTMLButtonElement>(`[data-brand-url="${PROJECT_URL}"]`)!.click();
  const alert = await waitFor(() => page.querySelector<HTMLElement>('[role="alert"]'));
  check("外链失败行动提示", alert.textContent?.includes(PROJECT_URL) === true);
  failOpen = false;
  document.documentElement.dataset.regression = "pass";
  document.getElementById("brand-result")!.textContent = `${checks.length} 项品牌检查通过`;
}
void run().catch((error: unknown) => {
  document.documentElement.dataset.regression = "fail";
  document.getElementById("brand-result")!.textContent = String(error);
});
