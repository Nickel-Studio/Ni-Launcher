/**
 * 加载动画留白与页面居中验收入口。
 *
 * 启动：pnpm dev，然后访问
 *   /tests/loading-animation/regression.html?scenario=vanilla&theme=dark
 *
 * 场景（全部渲染真实页面组件，IPC 由本文件模拟）：
 *   vanilla   原版资源页加载态（.van-page--loading）
 *   community 社区资源页加载态（.mr-page--loading）
 *   mods      Mod 扫描加载态（ModsPanel）
 *   settings  设置页首屏加载态（.nw-loading--viewport）
 *   game      设置页「游戏」分区载入态（GamePanel）
 *   vsettings 实例设置首屏加载态
 *   versions  实例列表首屏加载态（.vs-root--loading）
 *   detail    社区详情页 inline 加载态（size="inline"）
 *
 * fixture 只负责「停在加载态」：目标命令返回永不 resolve 的 Promise，
 * 其余命令返回安全的空数据，避免页面进入错误态覆盖加载卡片。
 */
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/styles/shell.css";
import Resources from "../../src/pages/Resources";
import Settings from "../../src/pages/Settings";
import VersionSettings from "../../src/pages/VersionSettings";
import Versions from "../../src/pages/Versions";
import { ProjectDetailPage } from "../../src/components/community/ProjectDetailPage";
import { LoadingCard } from "../../src/components/ui";
import type { ModrinthHit } from "../../src/components/modrinth/types";
import { SettingsProvider } from "../../src/lib/settings";
import { setVersionFocus } from "../../src/lib/instances";

type Scenario =
  | "vanilla"
  | "community"
  | "mods"
  | "settings"
  | "game"
  | "vsettings"
  | "versions"
  | "detail"
  | "error";

const params = new URLSearchParams(window.location.search);
const scenario = (params.get("scenario") ?? "vanilla") as Scenario;
const theme = params.get("theme") === "light" ? "light" : "dark";

const SETTINGS = {
  themeMode: theme,
  accent: "mono",
  backgroundImage: null,
  backgroundBlur: 0,
  backgroundOpacity: 0.5,
  privacyMode: false,
  downloadSource: "official",
  versionListSource: "official_auto",
  fileDownloadSource: "official_auto",
  communityApiSource: "official_auto",
  modrinthApiMirror: "https://mod.mcimirror.top/modrinth/v2/",
  curseforgeApiMirror: "https://mod.mcimirror.top/curseforge/v1/",
  communityFileMirror: "https://mod.mcimirror.top/",
  maxDownloadThreads: 10,
  downloadLimitBps: 0,
  verifyMicrosoftSsl: true,
  offlineUsername: "FixturePlayer",
  accountType: "offline",
  littleskinServer: "https://littleskin.cn",
  memoryMb: 4096,
};

/** 永不 resolve：让目标页面稳定停在加载态。 */
function pending(): Promise<never> {
  return new Promise<never>(() => {});
}

const DETAIL_HIT: ModrinthHit = {
  slug: "fixture-project",
  title: "Fixture Project",
  description: "用于校验 inline 加载卡片",
  iconUrl: "",
  downloads: 0,
  author: "Nickel",
  projectType: "mod",
  versions: [],
  loaders: ["fabric"],
  dateModified: "2026-11-01T00:00:00Z",
  source: "modrinth",
};

const VERSION_DETAIL = {
  id: "fixture-instance",
  dir: "C:\\NoWheRE-loading-fixture\\.minecraft",
  name: "fixture-instance",
  type: "release",
  minecraftVersion: "1.21.1",
  loader: "fabric",
  loaderVersion: "0.16.0",
  hasJar: true,
  jarSizeBytes: 1024,
  favorite: false,
  icon: "fabric",
  description: "",
  category: "",
  releaseTime: "2026-11-01T00:00:00Z",
};

let callbackId = 0;

const mock = {
  invoke: async (command: string): Promise<unknown> => {
    switch (command) {
      case "settings_get":
        return scenario === "settings" ? pending() : JSON.stringify(SETTINGS);
      case "settings_set":
      case "version_settings_set":
      case "game_settings_set":
        return null;
      case "game_settings_get":
        return scenario === "game" ? pending() : "{}";
      case "list_remote_versions":
        return scenario === "vanilla" || scenario === "community"
          ? pending()
          : JSON.stringify({
              latest: { release: "1.21.1", snapshot: "25w45a" },
              versions: [],
            });
      case "modrinth_search":
      case "curseforge_search":
        return scenario === "community"
          ? pending()
          : JSON.stringify({ total: 0, hits: [] });
      case "version_mods":
      case "version_mod_list":
        return scenario === "mods" ? pending() : "[]";
      case "version_mod_meta":
        return scenario === "mods" ? pending() : null;
      case "version_mod_updates":
        return scenario === "mods" ? pending() : [];
      case "version_mod_icon":
        return null;
      case "version_detail":
        return scenario === "vsettings" ? pending() : VERSION_DETAIL;
      case "version_settings_get":
        return "{}";
      case "cf_config_status":
        return JSON.stringify({ configured: true, source: "official_auto" });
      case "instances_get":
        return scenario === "versions"
          ? pending()
          : JSON.stringify({ active: null, roots: [] });
      case "default_minecraft_dir":
      case "resource_instance_context":
        return null;
      case "resources_scan":
      case "app_tasks_snapshot":
      case "java_detect":
        return "[]";
      case "plugin:event|listen":
      case "plugin:event|unlisten":
        return ++callbackId;
      default:
        return null;
    }
  },
  transformCallback: () => ++callbackId,
  unregisterCallback: () => {},
  convertFileSrc: (value: string) => value,
};

Object.assign(window, {
  __TAURI_INTERNALS__: mock,
  __TAURI_EVENT_PLUGIN_INTERNALS__: {
    unregisterListener: () => {},
  },
});

window.localStorage.setItem("nowhere.settings", JSON.stringify(SETTINGS));
document.documentElement.dataset.theme = theme;
document.documentElement.dataset.accent = "mono";
document.documentElement.classList.add("nw-app-ready");

/** 真实 PagerLayout 的页面容器由各页面组件自带，这里只补外壳。 */
function Shell({ children }: { children: ReactNode }) {
  return (
    <div className="nw-shell">
      <div className="nw-titlebar">
        <div className="nw-titlebar__left">
          <span className="nw-brand">
            <span className="nw-brand__text">Nickel</span>
          </span>
        </div>
      </div>
      <div className="nw-body">
        <main className="nw-content">
          <div className="nw-swap">
            <div className="nw-swap__layer nw-swap__layer--in">{children}</div>
          </div>
        </main>
      </div>
    </div>
  );
}

/** 点击侧边栏项（按可见文案精确匹配）。 */
function clickNav(label: string): boolean {
  const item = Array.from(
    document.querySelectorAll<HTMLButtonElement>(".nw-pager__item")
  ).find((node) => node.textContent?.trim() === label);
  if (!item) return false;
  item.click();
  return true;
}

/** 等待目标加载态出现并稳定一帧后打上 ready 标记。 */
function watchReady() {
  let navClicked = false;
  let tries = 0;
  const target = (() => {
    if (scenario === "vanilla") return ".van-page--loading .nw-loading";
    if (scenario === "community") return ".mr-page--loading .nw-loading";
    if (scenario === "mods") return ".vst-mods .nw-loading";
    if (scenario === "settings") return ".nw-loading--viewport";
    if (scenario === "game") return ".nw-pager__page .nw-loading";
    if (scenario === "versions") return ".vs-root--loading .nw-loading";
    if (scenario === "detail") return ".pd-downloads .nw-loading";
    if (scenario === "error") return ".nw-loading--error";
    return ".nw-pager__page .nw-loading";
  })();

  const tick = () => {
    tries += 1;
    if (
      scenario === "community" ||
      scenario === "mods" ||
      scenario === "game"
    ) {
      if (!navClicked && document.querySelector(".nw-pager__item")) {
        navClicked = clickNav(
          scenario === "community"
            ? "Mod"
            : scenario === "mods"
            ? "Mod 管理"
            : "游戏"
        );
      }
    }
    const loader = document.querySelector(target);
    if (loader) {
      window.requestAnimationFrame(() => {
        window.requestAnimationFrame(() => {
          document.documentElement.dataset.ready = "true";
          document.documentElement.dataset.scenario = scenario;
        });
      });
      return;
    }
    if (tries < 600) window.setTimeout(tick, 50);
  };
  tick();
}

if (scenario === "mods" || scenario === "vsettings") {
  setVersionFocus({
    root: "C:\\NoWheRE-loading-fixture\\.minecraft",
    versionId: "fixture-instance",
  });
}

const root = createRoot(document.getElementById("root")!);
root.render(
  <SettingsProvider>
    <Shell>
      {scenario === "error" ? (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            minHeight: "70vh",
          }}
        >
          <LoadingCard state="error" label="网络故障" />
        </div>
      ) : scenario === "settings" || scenario === "game" ? (
        <Settings />
      ) : scenario === "mods" || scenario === "vsettings" ? (
        <VersionSettings />
      ) : scenario === "versions" ? (
        <Versions />
      ) : scenario === "detail" ? (
        <ProjectDetailPage
          kind="mod"
          hit={DETAIL_HIT}
          adapter={{
            openProject: async () => null,
            fetchVersions: () => pending(),
            install: async () => null,
          }}
          instance={{
            root: "C:\\NoWheRE-loading-fixture\\.minecraft",
            versionId: "fixture-instance",
            dir: "C:\\NoWheRE-loading-fixture\\.minecraft",
            label: "fixture-instance",
            mcVersion: "1.21.1",
            loader: "fabric",
          }}
          onBack={() => {}}
        />
      ) : (
        <Resources />
      )}
    </Shell>
  </SettingsProvider>
);

/** 在 React 首次提交后再开始轮询，避免抢在页面挂载前查询加载态。 */
window.setTimeout(watchReady, 0);
