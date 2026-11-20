/**
 * 实例操作与 More 指南浏览器回归。
 *
 * 启动：pnpm dev -- --port 1504，然后打开
 * /tests/version-instances/regression.html
 *
 * 该页挂载真实 Versions / More 组件，并用固定 IPC 夹具验证整卡选择、返回首页、
 * 设置跳转、收藏、删除二次确认与指南真实入口。
 * `?visual=settings|overview` 额外挂载真实的实例设置 / 概览面板，
 * 供 `run-ui-capture.mjs` 在 900×600 下做明暗与布局截图。
 */
import { createRoot } from "react-dom/client";
import type { ReactNode } from "react";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";
import "../../src/pages/VersionSettings.css";
import { SettingsProvider } from "../../src/lib/settings";
import {
  getLaunchInstance,
  getVersionFocus,
  setLaunchInstance,
  setVersionFocus,
} from "../../src/lib/instances";
import { getRoute, setRoute } from "../../src/lib/nav";
import Versions from "../../src/pages/Versions";
import More from "../../src/pages/More";
import {
  OverviewPanel,
  SettingsPanel,
  type OverviewData,
} from "../../src/components/version/vsettings/panels";
import type { InstanceEntry } from "../../src/components/version/instancesApi";

const ROOT = "C:\\Nickel-regression\\.minecraft";

/** 实例设置视觉检查固定夹具：覆盖继承、显式清空与覆盖三种状态。 */
const VERSION_SETTINGS_FIXTURE = {
  versionIsolation: null,
  windowTitle: null,
  customInfo: "",
  javaPath: null,
  launcherVisibility: null,
  processPriority: null,
  windowMode: null,
  windowWidth: null,
  windowHeight: null,
  memoryMode: "custom",
  memoryMb: 6144,
  memoryManagement: null,
  highPerformanceGpu: null,
  offlineSkin: null,
  offlineSkinName: null,
  offlineSkinPath: null,
  offlineSkinSlim: null,
  serverLogin: "thirdParty",
  authServer: "https://littleskin.cn/api/yggdrasil",
  serverRegistration: null,
  serverName: "",
  serverAddress: null,
  jvmArgs: null,
  gameArgs: null,
  preLaunchCommand: "",
  noModUpdates: null,
  disableModUpdates: null,
  disableFileCheck: false,
  disableWrapper: null,
  disableLwjglAgent: true,
  icon: "fabric",
};

/** 全局设置夹具：用于展示「跟随全局」时的实际来源值。 */
const GLOBAL_GAME_SETTINGS_FIXTURE = {
  gameDir: ROOT,
  javaPath: "",
  versionIsolation: true,
  windowTitle: "全局窗口标题",
  customInfo: "全局自定义信息",
  launcherVisibility: "keep",
  processPriority: "normal",
  windowMode: "default",
  windowWidth: 854,
  windowHeight: 480,
  memoryMode: "auto",
  memoryMb: 6144,
  memoryManagement: "g1-tuned",
  highPerformanceGpu: false,
  disableWrapper: false,
  disableLwjglAgent: false,
  offlineSkin: "random",
  offlineSkinName: "",
  offlineSkinPath: "",
  offlineSkinSlim: false,
  jvmArgs: "-Xmx6G",
  gameArgs: "",
  preLaunchCommand: "echo 全局",
  serverLogin: "any",
  authServer: "",
  serverRegistration: "",
  serverName: "",
  serverAddress: "",
};

const JAVA_FIXTURE = [
  {
    path: "C:\\Java\\jdk-21\\bin\\java.exe",
    version: "21.0.4",
    vendor: "Eclipse Adoptium",
    isDefault: true,
    major: 21,
    bits: 64,
    architecture: "x86_64",
    executable: true,
  },
];

const OVERVIEW_FIXTURE: OverviewData = {
  id: "26.3-fabric",
  dir: `${ROOT}\\versions\\26.3-fabric`,
  type: "release",
  loader: "fabric",
  loaderVersion: "0.17.0",
  icon: "fabric",
  name: "Fabric 26.3",
  description: "用于实例页视觉检查的固定实例。",
  favorite: true,
  category: "",
  hasJar: true,
  jarSizeBytes: 21_000_000,
  releaseTime: "2026-11-10T00:00:00+00:00",
};

type RegressionCheck = {
  name: string;
  ok: boolean;
  detail: string;
};

type FixtureState = {
  status: "running" | "pass" | "fail";
  checks: RegressionCheck[];
};

type TauriMock = {
  invoke: (
    command: string,
    args?: Record<string, unknown>,
    options?: unknown
  ) => Promise<unknown>;
  transformCallback: (
    callback: (payload: unknown) => void,
    once?: boolean
  ) => number;
  unregisterCallback: (id: number) => void;
  convertFileSrc: (path: string) => string;
};

declare global {
  interface Window {
    __TAURI_INTERNALS__: TauriMock;
    __TAURI_EVENT_PLUGIN_INTERNALS__: {
      unregisterListener: (event: string, eventId: number) => void;
    };
    __versionInstancesRegression: FixtureState;
  }
}

const calls = {
  invoke: [] as Array<{ command: string; args: Record<string, unknown> }>,
  dialogs: [] as string[],
  deletes: [] as Array<{ dir: string; versionId: string }>,
  renames: [] as Array<Record<string, unknown>>,
};
const fixtureState: FixtureState = { status: "running", checks: [] };
window.__versionInstancesRegression = fixtureState;

let dialogResult: "Ok" | "Cancel" = "Cancel";
let callbackId = 0;
let entries: InstanceEntry[] = [
  {
    id: "26.3-fabric",
    displayName: "Fabric 26.3",
    favorite: true,
    favoriteAt: 1700000000000,
    category: "",
    type: "release",
    loader: "fabric",
    loaderVersion: "0.17.0",
    minecraftVersion: "26.3",
    namedProfile: true,
    icon: "fabric",
    path: `${ROOT}\\versions\\26.3-fabric`,
    releaseTime: "2026-11-10T00:00:00+00:00",
    hasJson: true,
  },
  {
    id: "26.2-vanilla",
    displayName: "原版 26.2",
    favorite: false,
    favoriteAt: null,
    category: "",
    type: "release",
    loader: "vanilla",
    loaderVersion: null,
    minecraftVersion: "26.2",
    namedProfile: false,
    icon: "grass",
    path: `${ROOT}\\versions\\26.2-vanilla`,
    releaseTime: "2026-11-09T00:00:00+00:00",
    hasJson: true,
  },
  {
    id: "broken-instance",
    displayName: "损坏实例",
    favorite: false,
    favoriteAt: null,
    category: "",
    type: "unknown",
    loader: "unknown",
    loaderVersion: null,
    minecraftVersion: null,
    namedProfile: false,
    icon: "cobblestone",
    path: `${ROOT}\\versions\\broken-instance`,
    releaseTime: null,
    hasJson: false,
  },
];

let registry = {
  roots: [ROOT],
  active: ROOT,
  labels: {} as Record<string, string>,
};

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function fixtureEntry(versionId: string): InstanceEntry | undefined {
  return entries.find((entry) => entry.id === versionId);
}

function versionDetail(versionId: string): unknown {
  const entry = fixtureEntry(versionId);
  if (!entry) return null;
  return {
    id: entry.id,
    minecraftVersion: entry.minecraftVersion,
    dir: ROOT,
    type: entry.type,
    loader: entry.loader,
    loaderVersion: entry.loaderVersion,
    icon: entry.icon,
    name: entry.displayName,
    description: "",
    favorite: entry.favorite,
    category: entry.category,
    hasJar: entry.hasJson,
    jarSizeBytes: 1024,
    releaseTime: entry.releaseTime,
  };
}

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};
window.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    calls.invoke.push({ command, args });
    if (command === "settings_get") {
      return JSON.stringify({ themeMode: "dark", accent: "mono" });
    }
    if (command === "settings_set") return null;
    if (command === "game_settings_get") {
      return JSON.stringify(GLOBAL_GAME_SETTINGS_FIXTURE);
    }
    if (command === "game_settings_set") return null;
    if (command === "version_settings_get") {
      return JSON.stringify(VERSION_SETTINGS_FIXTURE);
    }
    if (command === "version_settings_set") return null;
    if (command === "version_icon_pick") return null;
    if (command === "open_folder") return null;
    if (command === "game_runtime_info") {
      return {
        totalMemoryMb: 16384,
        availableMemoryMb: 10240,
        suggestedMemoryMb: 4096,
        capabilities: {
          windowTitle: true,
          launchWrapper: true,
          lwjglAgent: true,
        },
      };
    }
    if (command === "resolved_memory_plan") {
      return {
        totalMemoryMb: 16384,
        availableMemoryMb: 10240,
        suggestedMemoryMb: 4096,
        modCount: 12,
      };
    }
    if (command === "java_detect") return JAVA_FIXTURE;
    if (command === "java_resolve") {
      return {
        path: JAVA_FIXTURE[0].path,
        source: "auto",
        version: JAVA_FIXTURE[0].version,
        vendor: JAVA_FIXTURE[0].vendor,
        major: JAVA_FIXTURE[0].major,
        bits: JAVA_FIXTURE[0].bits,
        compatible: true,
        needsDownload: false,
        reason: "",
        requirement: {
          minMajor: 21,
          maxMajor: null,
          preferredMajor: 21,
          reason: "",
        },
      };
    }
    if (command === "instances_get") return JSON.stringify(registry);
    if (command === "instances_set") {
      if (typeof args.json === "string") {
        const parsed: unknown = JSON.parse(args.json);
        if (isRecord(parsed)) {
          registry = {
            roots: Array.isArray(parsed.roots)
              ? parsed.roots.filter(
                  (item): item is string => typeof item === "string"
                )
              : [],
            active: typeof parsed.active === "string" ? parsed.active : "",
            labels: isRecord(parsed.labels)
              ? Object.fromEntries(
                  Object.entries(parsed.labels).filter(
                    (item): item is [string, string] =>
                      typeof item[1] === "string"
                  )
                )
              : {},
          };
        }
      }
      return null;
    }
    if (command === "instance_ensure_default") return ROOT;
    if (command === "instance_scan") return JSON.stringify(entries);
    if (command === "version_detail") {
      return JSON.stringify(
        versionDetail(typeof args.versionId === "string" ? args.versionId : "")
      );
    }
    if (command === "version_rename") {
      calls.renames.push(args);
      const versionId =
        typeof args.versionId === "string" ? args.versionId : "";
      const name = typeof args.name === "string" ? args.name : "";
      const favorite = args.favorite === true;
      entries = entries.map((entry) =>
        entry.id === versionId
          ? {
              ...entry,
              displayName: name || entry.displayName,
              favorite,
              favoriteAt: favorite ? Date.now() : null,
            }
          : entry
      );
      return null;
    }
    if (command === "version_delete") {
      const versionId =
        typeof args.versionId === "string" ? args.versionId : "";
      calls.deletes.push({
        dir: typeof args.dir === "string" ? args.dir : "",
        versionId,
      });
      entries = entries.filter((entry) => entry.id !== versionId);
      return null;
    }
    if (command === "plugin:dialog|message") {
      calls.dialogs.push(
        typeof args.message === "string" ? args.message : "message"
      );
      return dialogResult;
    }
    if (command === "plugin:dialog|open") return null;
    if (command === "app_tasks_snapshot") return [];
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return null;
    return null;
  },
  transformCallback: () => ++callbackId,
  unregisterCallback: () => {},
  convertFileSrc: (path) => path,
};

document.documentElement.dataset.regression = "running";
document.body.style.overflow = "auto";
document.documentElement.style.overflow = "auto";

function required<T extends Element>(root: ParentNode, selector: string): T {
  const node = root.querySelector<T>(selector);
  if (!node) throw new Error(`找不到元素：${selector}`);
  return node;
}

function pushCheck(name: string, ok: boolean, detail = ""): void {
  fixtureState.checks.push({ name, ok, detail });
}

function expect(name: string, condition: boolean, detail = ""): void {
  pushCheck(name, condition, detail);
  if (!condition) throw new Error(`${name}${detail ? `：${detail}` : ""}`);
}

async function waitFor<T>(
  probe: () => T | null | false | undefined,
  label: string,
  timeout = 6000
): Promise<T> {
  const deadline = performance.now() + timeout;
  while (performance.now() < deadline) {
    const value = probe();
    if (value) return value;
    await sleep(25);
  }
  throw new Error(`等待超时：${label}`);
}

function renderResult(): void {
  const result = document.querySelector<HTMLElement>("#regression-result");
  if (!result) return;
  const failed = fixtureState.checks.filter((check) => !check.ok);
  fixtureState.status = failed.length ? "fail" : "pass";
  result.dataset.status = fixtureState.status;
  result.textContent = fixtureState.checks
    .map(
      (check) =>
        `${check.ok ? "PASS" : "FAIL"}  ${check.name}${
          check.detail ? ` · ${check.detail}` : ""
        }`
    )
    .join("\n");
  document.documentElement.dataset.regression = fixtureState.status;
}

const appRoot = createRoot(document.getElementById("root") as HTMLElement);

function render(node: ReactNode): void {
  appRoot.render(<SettingsProvider>{node}</SettingsProvider>);
}

function card(id: string): HTMLElement {
  return required<HTMLElement>(
    document,
    `[data-instance-id="${CSS.escape(id)}"]`
  );
}

async function runVersionsRegression(): Promise<void> {
  setLaunchInstance(null);
  setVersionFocus(null);
  setRoute("instances");
  render(<Versions />);
  await waitFor(
    () => document.querySelectorAll(".vs-card").length === 3,
    "卡片"
  );

  const selectCard = card("26.3-fabric");
  const selectButton = required<HTMLButtonElement>(
    selectCard,
    ".vs-card__select"
  );
  selectButton.click();
  await waitFor(
    () => getLaunchInstance()?.versionId === "26.3-fabric",
    "整卡选择实例"
  );
  expect("选择实例后返回首页", getRoute() === "home", getRoute());
  expect(
    "损坏实例不能作为启动实例",
    required<HTMLButtonElement>(card("broken-instance"), ".vs-card__select")
      .disabled
  );
  expect(
    "旧版不再有独立主操作按钮",
    selectCard.querySelector(".vs-card__primary") === null
  );

  const favorite = required<HTMLButtonElement>(
    card("26.2-vanilla"),
    ".vs-card__favorite"
  );
  favorite.click();
  await waitFor(() => calls.renames.length === 1, "收藏写入");
  expect(
    "收藏是独立入口并真实写回",
    calls.renames[0]?.favorite === true,
    JSON.stringify(calls.renames[0])
  );

  const settingsButton = required<HTMLButtonElement>(
    card("26.3-fabric"),
    'button[aria-label^="实例设置 "]'
  );
  expect("设置使用 SVG 图标", settingsButton.querySelector("svg") !== null);
  settingsButton.click();
  await waitFor(() => getRoute() === "versionSettings", "设置跳转");
  expect(
    "版本设置携带根目录与实例身份",
    getVersionFocus()?.root === ROOT &&
      getVersionFocus()?.versionId === "26.3-fabric"
  );
  setRoute("instances");

  dialogResult = "Cancel";
  const beforeCancelDialogs = calls.dialogs.length;
  required<HTMLButtonElement>(
    card("26.3-fabric"),
    'button[aria-label^="删除实例 "]'
  ).click();
  await waitFor(
    () => calls.dialogs.length === beforeCancelDialogs + 1,
    "删除确认框"
  );
  await sleep(80);
  expect(
    "删除先弹二次确认",
    calls.dialogs[beforeCancelDialogs]?.includes("删除实例") === true
  );
  expect("取消确认不删除实例", calls.deletes.length === 0);
  expect(
    "取消删除后实例仍存在",
    document.querySelector('[data-instance-id="26.3-fabric"]') !== null
  );

  dialogResult = "Ok";
  required<HTMLButtonElement>(
    card("26.3-fabric"),
    'button[aria-label^="删除实例 "]'
  ).click();
  await waitFor(
    () => document.querySelector('[data-instance-id="26.3-fabric"]') === null,
    "确认后删除卡片"
  );
  expect(
    "确认后真实调用删除命令",
    calls.deletes.length === 1 && calls.deletes[0]?.versionId === "26.3-fabric"
  );
  expect("删除当前启动实例后清空选择", getLaunchInstance() === null);
}

async function buttonByText(label: string): Promise<HTMLButtonElement> {
  return waitFor(
    () =>
      Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent?.trim() === label
      ) ?? null,
    `按钮 ${label}`
  );
}

async function runMoreRegression(): Promise<void> {
  setRoute("more");
  render(<More />);
  await waitFor(
    () => document.querySelectorAll(".mo-card__action").length === 4,
    "指南卡片"
  );

  const text = document.body.textContent ?? "";
  expect(
    "指南不再声称再次点击可取消选择",
    !text.includes("再次点击同一张卡片则取消选择")
  );
  expect(
    "指南不再引用不存在的头像切换按钮",
    !text.includes("头像上的切换按钮打开账号管理")
  );
  expect("指南不再使用不存在的下载与游戏分区", !text.includes("下载与游戏"));
  expect(
    "指南不再把全局与实例内存混为同一个值",
    !text.includes("改的是同一个值")
  );

  const routes: Array<
    [string, "home" | "instances" | "resources" | "settings"]
  > = [
    ["回到首页登录", "home"],
    ["打开实例管理", "instances"],
    ["打开资源页", "resources"],
    ["打开设置", "settings"],
  ];
  for (const [label, route] of routes) {
    (await buttonByText(label)).click();
    await waitFor(() => getRoute() === route, `${label} 跳转`);
    expect(`指南按钮直达 ${label}`, getRoute() === route, getRoute());
    setRoute("more");
  }

  expect(
    "指南每卡只有一个主入口",
    document.querySelectorAll(".mo-card__action").length === 4
  );
  expect(
    "指南没有占位链接",
    document.querySelectorAll(".mo-page a").length === 0
  );
}

function visualMode(): "instances" | "more" | "settings" | "overview" | null {
  const mode = new URLSearchParams(window.location.search).get("visual");
  return mode === "instances" ||
    mode === "more" ||
    mode === "settings" ||
    mode === "overview"
    ? mode
    : null;
}

async function start(): Promise<void> {
  const mode = visualMode();
  if (mode) {
    setLaunchInstance({
      root: ROOT,
      versionId: "26.3-fabric",
      displayName: "Fabric 26.3",
      loader: "fabric",
      icon: "fabric",
    });
    setRoute(
      mode === "instances"
        ? "instances"
        : mode === "more"
        ? "more"
        : "versionSettings"
    );
    render(
      <div className="fixture-wrap">
        {mode === "instances" ? (
          <Versions />
        ) : mode === "more" ? (
          <More />
        ) : mode === "settings" ? (
          <div className="vst-root">
            <SettingsPanel
              root={ROOT}
              versionId="26.3-fabric"
              javas={JAVA_FIXTURE}
            />
          </div>
        ) : (
          <div className="vst-root">
            <OverviewPanel
              data={OVERVIEW_FIXTURE}
              root={ROOT}
              onChanged={() => undefined}
            />
          </div>
        )}
      </div>
    );
    const result = document.querySelector<HTMLElement>("#regression-result");
    if (result) {
      result.textContent = "视觉检查模式：回归脚本未运行";
      result.dataset.status = "visual";
    }
    document.documentElement.dataset.regression = "visual";
    return;
  }

  try {
    await runVersionsRegression();
    await runMoreRegression();
  } catch (error) {
    pushCheck(
      "回归执行异常",
      false,
      error instanceof Error ? error.message : String(error)
    );
  } finally {
    renderResult();
  }
}

void start();
