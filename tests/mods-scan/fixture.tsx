/** 真实组件 + 可控 IPC；运行 pnpm dev 后访问 /tests/mods-scan/regression.html。 */
import { StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";
import "../../src/pages/VersionSettings.css";
import { SettingsProvider } from "../../src/lib/settings";
import { setVersionFocus } from "../../src/lib/instances";
import VersionSettings from "../../src/pages/VersionSettings";
import { GamePanel } from "../../src/components/settings/GamePanel";
import { SettingsPanel } from "../../src/components/version/vsettings/panels";
import { ModsPanel } from "../../src/components/version/vsettings/ModsPanel";
import {
  MOD_LIST_TIMEOUT_MS,
  MOD_META_TIMEOUT_MS,
  MOD_POLICY_TIMEOUT_MS,
  MOD_UPDATE_TIMEOUT_MS,
} from "../../src/components/version/vsettings/useMods";
import type {
  ModEntry,
  ModMeta,
} from "../../src/components/version/vsettings/mods";

const ROOT = "C:\\NoWheRE-mods-regression\\.minecraft";
const SECOND_ROOT = "C:\\NoWheRE-mods-regression\\second";
const VERSION = "命名实例";
const ICON_DATA_URL =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='32' height='32'%3E%3Crect width='32' height='32' rx='6' fill='%2333cc66'/%3E%3C/svg%3E";
const checks: Array<{ name: string; ok: boolean; detail: string }> = [];
const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
type Handler = (args: Record<string, unknown>) => Promise<unknown>;
type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
};

// 列表 / 元数据 / 更新 / 图标分成四条可控通道，验证渐进加载互不阻塞。
let listHandler: Handler = async () => [];
let metaHandler: Handler = async () => null;
let updateHandler: Handler = async () => [];
let actionHandler: Handler = async () => null;
let iconHandler: Handler = async () => null;
let settingsHandler: Handler = async () => ({});
let globalSettings: Record<string, unknown> = {
  memoryMb: 4096,
  windowTitle: "",
  customInfo: "",
};
let titleSupported = true;
let callbackId = 0;
let fastTimeout = false;
const originalTimeout = window.setTimeout.bind(window);
window.setTimeout = ((
  handler: TimerHandler,
  timeout?: number,
  ...args: unknown[]
) =>
  originalTimeout(
    handler,
    fastTimeout &&
      (timeout === MOD_LIST_TIMEOUT_MS ||
        timeout === MOD_META_TIMEOUT_MS ||
        timeout === MOD_POLICY_TIMEOUT_MS ||
        timeout === MOD_UPDATE_TIMEOUT_MS)
      ? 80
      : timeout,
    ...args
  )) as typeof window.setTimeout;

/** 首屏占位条目：名称与简介相同，版本/图标/标签均为空。 */
function mod(
  name: string,
  dir = ROOT,
  overrides: Partial<ModEntry> = {}
): ModEntry {
  const path = `${dir}\\versions\\${VERSION}\\mods\\${name}.jar`;
  return {
    path,
    physicalPaths: [path],
    name,
    description: name,
    version: "",
    enabled: true,
    updateAvailable: false,
    duplicateGroup: null,
    tags: [],
    iconPath: null,
    metaLoaded: false,
    ...overrides,
  };
}

function modMeta(name: string, overrides: Partial<ModMeta> = {}): ModMeta {
  return {
    name,
    description: `${name} 的真实简介`,
    version: "1.0",
    tags: [],
    iconPath: null,
    duplicateGroup: `name:${name.toLowerCase()}`,
    ...overrides,
  };
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

const runtime = () => ({
  totalMemoryMb: 16384,
  availableMemoryMb: 10000,
  suggestedMemoryMb: 4096,
  capabilities: {
    windowTitle: titleSupported,
    memoryOptimize: true,
    processPriority: true,
    offlineSkin: true,
    launchWrapper: true,
    lwjglAgent: true,
    highPerformanceGpu: true,
  },
});
const mock = {
  invoke: async (
    command: string,
    args: Record<string, unknown> = {}
  ): Promise<unknown> => {
    calls.push({ command, args });
    if (command === "settings_get")
      return JSON.stringify({ themeMode: "dark", accent: "mono" });
    if (command === "settings_set" || command === "version_settings_set")
      return null;
    if (command === "game_settings_get") return JSON.stringify(globalSettings);
    if (command === "game_settings_set") {
      globalSettings = JSON.parse(String(args.json));
      return null;
    }
    if (command === "version_settings_get") return settingsHandler(args);
    if (command === "version_mod_list") return listHandler(args);
    if (command === "version_mod_meta") return metaHandler(args);
    if (command === "version_mod_updates") return updateHandler(args);
    // 旧整表命令只保留兼容路由，新前端不应调用它。
    if (command === "version_mods") return listHandler(args);
    if (command === "version_mods_action") return actionHandler(args);
    if (command === "version_mod_icon") return iconHandler(args);
    if (command === "version_detail")
      return {
        id: args.versionId,
        dir: args.dir,
        name: args.versionId,
        type: "release",
        minecraftVersion: "26.3",
        loader: "fabric",
        loaderVersion: "0.18.0",
        hasJar: true,
        jarSizeBytes: 1024,
        favorite: false,
        icon: "fabric",
        description: "",
        category: "",
        releaseTime: "2026-11-01T00:00:00Z",
      };
    if (command === "java_detect" || command === "app_tasks_snapshot")
      return [];
    if (command === "java_resolve")
      return {
        path: null,
        source: "unavailable",
        version: "",
        vendor: "",
        major: null,
        bits: null,
        compatible: false,
        needsDownload: true,
        reason: "启动时下载 Java 25",
        requirement: {
          minMajor: 25,
          maxMajor: null,
          preferredMajor: 25,
          reason: "Minecraft 26.3",
        },
      };
    if (command === "game_runtime_info") return runtime();
    if (command === "resolved_memory_plan")
      return { ...runtime(), modCount: 2 };
    if (command === "plugin:event|listen") return ++callbackId;
    if (command === "plugin:event|unlisten" || command === "open_folder")
      return null;
    if (command === "plugin:dialog|message") return "Ok";
    throw new Error(`未模拟命令：${command}`);
  },
  transformCallback: () => ++callbackId,
  unregisterCallback: () => {},
  convertFileSrc: (value: string) => value,
};
Object.assign(window, {
  __TAURI_INTERNALS__: mock,
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
  __modsRegression: { checks, calls },
});

const app = createRoot(document.getElementById("root")!);
function render(node: ReactNode) {
  app.render(
    <SettingsProvider>
      <div className="fixture-wrap">{node}</div>
    </SettingsProvider>
  );
}
const sleep = (ms: number) =>
  new Promise<void>((resolve) => originalTimeout(resolve, ms));
function expect(name: string, ok: boolean, detail = "") {
  checks.push({ name, ok, detail });
  if (!ok) throw new Error(`${name}：${detail}`);
}
async function waitFor(probe: () => boolean, name: string, timeout = 5000) {
  const deadline = performance.now() + timeout;
  while (performance.now() < deadline) {
    if (probe()) return;
    await sleep(16);
  }
  throw new Error(`等待超时：${name}`);
}
function button(label: string): HTMLButtonElement {
  const found = [
    ...document.querySelectorAll<HTMLButtonElement>("button"),
  ].find(
    (node) =>
      node.getAttribute("aria-label") === label ||
      node.textContent?.trim() === label
  );
  if (!found) throw new Error(`找不到按钮：${label}`);
  return found;
}
function mountMods(root = ROOT, versionId = VERSION, strict = false) {
  const panel = (
    <ModsPanel
      root={root}
      versionId={versionId}
      displayName={versionId}
      onBrowse={() => {}}
      onSettings={() => {}}
    />
  );
  render(strict ? <StrictMode>{panel}</StrictMode> : panel);
}
async function freshMods(root = ROOT, versionId = VERSION, strict = false) {
  render(null);
  await sleep(30);
  mountMods(root, versionId, strict);
  await sleep(30);
}
const names = () =>
  [...document.querySelectorAll(".vst-mod__name")]
    .map((node) => node.textContent ?? "")
    .join("|");
const descriptions = () =>
  [...document.querySelectorAll(".vst-mod__description")]
    .map((node) => node.textContent ?? "")
    .join("|");
const rowPaths = () =>
  [...document.querySelectorAll<HTMLElement>("li.vst-mod")].map(
    (node) => node.dataset.path ?? ""
  );
const iconImages = () => [
  ...document.querySelectorAll<HTMLImageElement>(".vst-mod__icon img"),
];
const loading = () =>
  document
    .querySelector('[aria-label="Mod 列表"]')
    ?.getAttribute("aria-busy") === "true";
const alert = () => document.querySelector('[role="alert"]')?.textContent ?? "";
const errorCard = () =>
  document.querySelector<HTMLElement>(".vst-mods .nw-loading--error");
const errorLabel = () =>
  errorCard()?.querySelector(".nw-loading__label")?.textContent?.trim() ?? "";

async function run() {
  // ── 1. 首屏占位：文件名 + 同名简介 + 透明占位图 ──
  const first = Array.from({ length: 10 }, (_, index) =>
    mod(`Placeholder ${index}`)
  );
  const gate = deferred<unknown>();
  const releaseRest = deferred<unknown>();
  const updatesPending = deferred<unknown>();
  const metaPending = new Map<string, Deferred<unknown>>();
  listHandler = async () => first;
  // 更新检查在全部元数据完成后才开始；先挂起，验证逐行收尾后仍可继续操作。
  updateHandler = async () => updatesPending.promise;
  metaHandler = async (args) => {
    await gate.promise;
    const index = first.findIndex((item) => item.path === args.path);
    if (index >= 4) {
      await releaseRest.promise;
      return modMeta(`真实 ${index}`);
    }
    const item = deferred<unknown>();
    metaPending.set(String(args.path), item);
    return item.promise;
  };
  await freshMods();
  await waitFor(
    () => document.querySelectorAll(".vst-mod").length === 10 && !loading(),
    "首屏占位列表"
  );
  expect(
    "首屏立即显示文件名与同名简介",
    names() === first.map((item) => item.name).join("|") &&
      descriptions() === first.map((item) => item.name).join("|")
  );
  expect(
    "每个条目立即显示占位图",
    iconImages().length === 10 &&
      iconImages().every((image) => image.dataset.placeholder === "true")
  );
  const iconStyle = getComputedStyle(
    document.querySelector<HTMLElement>(".vst-mod__icon")!
  );
  expect(
    "占位图容器透明无框",
    iconStyle.backgroundColor === "rgba(0, 0, 0, 0)" &&
      iconStyle.borderRadius === "0px" &&
      iconStyle.borderTopWidth === "0px" &&
      iconStyle.boxShadow === "none" &&
      getComputedStyle(iconImages()[0]).objectFit === "contain" &&
      getComputedStyle(iconImages()[0]).imageRendering === "pixelated"
  );
  const beforeOrder = rowPaths();

  gate.resolve(null);
  await waitFor(() => metaPending.size >= 4, "元数据队列启动");
  const target = first[2];
  metaPending
    .get(target.path)!
    .resolve(modMeta("真实名称", { version: "2.0", tags: ["Magic"] }));
  await waitFor(() => names().includes("真实名称"), "单行元数据");
  expect(
    "元数据只更新对应行且顺序不变",
    rowPaths().join("|") === beforeOrder.join("|") &&
      names() ===
        first
          .map((item, index) => (index === 2 ? "真实名称2.0" : item.name))
          .join("|")
  );
  const updatedRow = [
    ...document.querySelectorAll<HTMLElement>("li.vst-mod"),
  ].find((row) => row.dataset.path === target.path)!;
  const animationName = getComputedStyle(
    updatedRow.querySelector(".vst-mod__text")!
  ).animationName;
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
  expect(
    "真实信息到达使用轻量淡入并尊重 reduced-motion",
    reducedMotion
      ? animationName === "none"
      : animationName === "vst-mod-meta-in"
  );

  const failed = first[1];
  metaPending.get(failed.path)!.reject(new Error("元数据损坏"));
  metaPending.get(first[0].path)!.resolve(modMeta("第一项"));
  metaPending.get(first[3].path)!.resolve(modMeta("第四项"));
  releaseRest.resolve(null);
  await waitFor(
    () =>
      document.querySelectorAll('.vst-mod__text[data-meta="true"]').length ===
      9,
    "其余元数据完成"
  );
  const failedRow = [
    ...document.querySelectorAll<HTMLElement>("li.vst-mod"),
  ].find((row) => row.dataset.path === failed.path)!;
  expect(
    "单条元数据失败保留占位且不产生整页错误",
    !alert() &&
      failedRow
        .querySelector(".vst-mod__name")
        ?.textContent?.startsWith("Placeholder 1") === true
  );

  // 全部元数据完成后才进入更新检查；失败静默，不提示也不打断列表。
  await waitFor(
    () => calls.some((call) => call.command === "version_mod_updates"),
    "更新检查开始"
  );
  updatesPending.reject(new Error("网络不可达"));
  await sleep(80);
  expect(
    "联网失败静默保留本地列表",
    names().includes("第一项") && !loading() && !alert()
  );
  expect(
    "不再显示正在检查更新",
    !document.body.textContent?.includes("正在检查 Mod 更新")
  );
  expect(
    "渐进加载不调用旧的整表扫描命令",
    calls.every((call) => call.command !== "version_mods")
  );

  // ── 2. 首屏 read_dir 失败显示错误动画与短文案 ──
  listHandler = async () => {
    throw new Error("目录访问被拒绝。");
  };
  metaHandler = async () => null;
  await freshMods();
  await waitFor(() => errorCard() !== null && !loading(), "扫描失败");
  expect(
    "目录失败使用错误动画与短文案",
    errorLabel() === "非法调用" &&
      errorCard()?.querySelector(".nw-loading__action button") !== null
  );
  expect("失败文案不出现连续句号", !errorLabel().includes("。。"));
  listHandler = async () => [mod("Retry")];
  (
    errorCard()?.querySelector<HTMLButtonElement>(
      ".nw-loading__action button"
    ) ?? button("重新扫描")
  ).click();
  await waitFor(() => names().includes("Retry") && !loading(), "扫描重试");
  expect(
    "重试真实重新发送列表命令",
    calls.filter((call) => call.command === "version_mod_list").length >= 2
  );

  listHandler = async () => "{bad-json";
  await freshMods();
  await waitFor(() => errorCard() !== null && !loading(), "损坏扫描响应");
  expect(
    "扫描解析异常归为文件损坏并解除 loading",
    errorLabel() === "文件损坏" && !loading()
  );

  const hanging = deferred<unknown>();
  fastTimeout = true;
  listHandler = async () => hanging.promise;
  await freshMods();
  await waitFor(() => errorCard() !== null && !loading(), "扫描超时");
  expect(
    "未响应 IPC 有界退出并归为网络故障",
    errorLabel() === "网络故障" && !errorLabel().includes("。。")
  );
  listHandler = async () => [mod("After timeout")];
  button("重新扫描").click();
  await waitFor(() => names().includes("After timeout"), "超时重试");
  hanging.resolve([mod("Late timeout")]);
  await sleep(60);
  expect("超时后迟到响应不能覆盖重试结果", !names().includes("Late timeout"));
  fastTimeout = false;

  // ── 3. 图标按需加载：占位图保留到真实图标返回 ──
  const iconPending = deferred<unknown>();
  const iconMod = mod("Icon mod");
  listHandler = async () => [iconMod];
  metaHandler = async () =>
    modMeta("Icon mod", { iconPath: "assets/icon.png" });
  iconHandler = async () => iconPending.promise;
  await freshMods();
  await waitFor(
    () => names().includes("Icon mod") && !loading(),
    "图标等待时列表"
  );
  expect(
    "图标未返回时保留占位图且不阻塞文件操作",
    iconImages()[0]?.dataset.placeholder === "true" &&
      !button("禁用 Icon mod").disabled &&
      !button("重新扫描 Mod").disabled
  );
  await waitFor(
    () => calls.some((call) => call.command === "version_mod_icon"),
    "图标请求开始"
  );
  iconPending.resolve(ICON_DATA_URL);
  await waitFor(
    () =>
      !!document.querySelector(".vst-mod__icon img:not([data-placeholder])"),
    "图标懒加载"
  );
  expect(
    "真实图标不使用像素化缩放",
    getComputedStyle(
      document.querySelector<HTMLImageElement>(
        ".vst-mod__icon img:not([data-placeholder])"
      )!
    ).imageRendering === "auto"
  );
  const iconRequest = calls.find((call) => call.command === "version_mod_icon");
  expect(
    "图标请求携带实例、文件与条目",
    !!iconRequest &&
      iconRequest.args.dir === ROOT &&
      iconRequest.args.versionId === VERSION &&
      iconRequest.args.path === iconMod.path &&
      iconRequest.args.iconEntry === "assets/icon.png"
  );

  const iconCalls = calls.filter(
    (call) => call.command === "version_mod_icon"
  ).length;
  listHandler = async () => [iconMod];
  button("重新扫描 Mod").click();
  await waitFor(() => names().includes("Icon mod") && !loading(), "图标重扫");
  await sleep(120);
  expect(
    "同一图标命中缓存不重复请求",
    calls.filter((call) => call.command === "version_mod_icon").length ===
      iconCalls
  );

  iconHandler = async () => {
    throw new Error("图标损坏");
  };
  listHandler = async () => [mod("Broken icon")];
  metaHandler = async () => modMeta("Broken icon", { iconPath: "bad.png" });
  await freshMods();
  await waitFor(
    () => names().includes("Broken icon") && !loading(),
    "坏图标列表"
  );
  await waitFor(
    () => calls.some((call) => call.command === "version_mod_icon"),
    "坏图标请求"
  );
  await sleep(80);
  expect(
    "图标失败不回传扫描错误且回退占位图",
    errorCard() === null &&
      iconImages()[0]?.dataset.placeholder === "true" &&
      !button("禁用 Broken icon").disabled
  );

  // ── 4. 整页失败：错误动画 + 居中短文案 + 重试入口 ──
  listHandler = async () => {
    throw new Error("布局测试失败");
  };
  await freshMods();
  await waitFor(() => errorCard() !== null && !loading(), "错误态布局");
  const card = errorCard()!;
  const label = card.querySelector<HTMLElement>(".nw-loading__label")!;
  const retry = card.querySelector<HTMLButtonElement>(
    ".nw-loading__action button"
  )!;
  const cardRect = card.getBoundingClientRect();
  const labelRect = label.getBoundingClientRect();
  expect(
    "错误文案收敛为「加载失败」",
    label.textContent?.trim() === "加载失败"
  );
  expect(
    "错误动画与重试入口水平居中",
    card.querySelector(".nw-loading__ore-x") !== null &&
      Math.abs(
        (labelRect.left + labelRect.right) / 2 -
          (cardRect.left + cardRect.right) / 2
      ) <= 1 &&
      retry.textContent?.trim() === "重新扫描" &&
      !retry.disabled
  );
  expect(
    "错误态不再堆叠长错误行",
    document.querySelector(".vst-mods__alert") === null
  );

  // ── 5. 设置读取不阻塞首屏；失败只提示设置问题 ──
  const policy = deferred<unknown>();
  settingsHandler = async () => policy.promise;
  listHandler = async () => [mod("Local without settings")];
  metaHandler = async () => null;
  await freshMods();
  await waitFor(
    () => names().includes("Local without settings") && !loading(),
    "设置未返回时本地列表"
  );
  expect(
    "设置等待不阻塞本地列表",
    !button("禁用 Local without settings").disabled
  );
  policy.reject(new Error("配置目录不可读"));
  await sleep(80);
  expect(
    "更新设置读取失败静默保留本地管理",
    !loading() && errorCard() === null && !alert().includes("配置目录不可读")
  );
  settingsHandler = async () => ({});

  // ── 6. 旧实例 / 旧版本 / 旧更新响应隔离 ──
  const old = deferred<unknown>();
  listHandler = async (args) =>
    args.dir === ROOT ? old.promise : [mod("Second root", SECOND_ROOT)];
  metaHandler = async () => null;
  await freshMods();
  mountMods(SECOND_ROOT);
  await waitFor(() => names().includes("Second root"), "切换目录");
  old.resolve([mod("Wrong root")]);
  await sleep(60);
  expect(
    "同版本不同根目录旧成功响应隔离",
    names().includes("Second root") &&
      !names().includes("Wrong root") &&
      !loading()
  );

  const staleFailure = deferred<unknown>();
  listHandler = async (args) =>
    args.versionId === VERSION ? staleFailure.promise : [mod("New version")];
  await freshMods();
  mountMods(ROOT, "新实例");
  await waitFor(() => names().includes("New version"), "切换实例");
  staleFailure.reject(new Error("旧实例扫描失败"));
  await sleep(60);
  expect(
    "旧实例失败不能污染当前错误与加载态",
    !alert().includes("旧实例") && !loading()
  );

  const oldUpdate = deferred<unknown>();
  updateHandler = async (args) =>
    args.versionId === VERSION ? oldUpdate.promise : [];
  listHandler = async (args) => [mod(String(args.versionId))];
  await freshMods();
  await waitFor(
    () => calls.some((call) => call.command === "version_mod_updates"),
    "旧更新开始"
  );
  mountMods(ROOT, "更新后实例");
  await waitFor(() => names().includes("更新后实例"), "更新中切换实例");
  oldUpdate.resolve([
    {
      path: mod("Wrong update", ROOT, { updateAvailable: true }).path,
      updateAvailable: true,
    },
  ]);

  await sleep(60);
  expect("后台更新旧响应不能覆盖新实例", !names().includes("Wrong update"));

  // ── 6.1 旧实例迟到的元数据不能覆盖新列表 ──
  const staleMeta = deferred<unknown>();
  let staleMetaCalls = 0;
  metaHandler = async (args) => {
    if (args.versionId === VERSION) {
      staleMetaCalls += 1;
      return staleMeta.promise;
    }
    return null;
  };
  listHandler = async (args) => [mod(String(args.versionId))];
  await freshMods();
  await waitFor(
    () => names().includes(VERSION) && !loading(),
    "旧实例元数据列表"
  );
  await waitFor(() => staleMetaCalls >= 1, "旧实例元数据请求");
  mountMods(ROOT, "新元数据实例");
  await waitFor(() => names().includes("新元数据实例"), "切换元数据实例");
  staleMeta.resolve(modMeta("旧实例迟到名称"));
  await sleep(60);
  expect(
    "旧实例迟到元数据不覆盖新列表",
    names() === "新元数据实例" && !names().includes("旧实例迟到名称")
  );

  // ── 6.2 实例切换后旧元数据队列停止取新任务 ──
  let queueMetaCalls = 0;
  metaHandler = async (args) => {
    if (args.versionId === "队列旧实例") {
      queueMetaCalls += 1;
      await sleep(5);
    }
    return null;
  };
  listHandler = async (args) =>
    args.versionId === "队列旧实例"
      ? Array.from({ length: 80 }, (_, index) =>
          mod(`队列 ${index}`, ROOT, {
            path: `${ROOT}\\versions\\队列旧实例\\mods\\队列 ${index}.jar`,
          })
        )
      : [mod(String(args.versionId))];
  await freshMods(ROOT, "队列旧实例");
  await waitFor(() => queueMetaCalls >= 4, "旧实例元数据队列");
  mountMods(ROOT, "队列新实例");
  await waitFor(() => names().includes("队列新实例"), "新实例列表");
  const callsAtSwitch = queueMetaCalls;
  await sleep(250);
  expect(
    "实例切换后旧元数据队列停止请求",
    queueMetaCalls < 40 && queueMetaCalls - callsAtSwitch < 20,
    `calls=${queueMetaCalls}, afterSwitch=${queueMetaCalls - callsAtSwitch}`
  );

  const action = deferred<unknown>();
  actionHandler = async () => action.promise;
  updateHandler = async () => [];
  listHandler = async (args) => [mod(String(args.versionId))];
  await freshMods();
  await waitFor(() => names().includes(VERSION) && !loading(), "动作测试列表");
  button(`禁用 ${VERSION}`).click();
  // 动作挂起期间切换实例：旧动作结束不能触发旧实例重扫。
  await sleep(30);
  mountMods(ROOT, "动作后实例");
  await waitFor(() => names().includes("动作后实例"), "文件动作时切换");
  const callCount = calls.filter(
    (call) =>
      call.command === "version_mod_list" && call.args.versionId === VERSION
  ).length;
  action.resolve(null);
  await sleep(60);
  expect(
    "旧文件动作结束不会触发旧实例重扫",
    calls.filter(
      (call) =>
        call.command === "version_mod_list" && call.args.versionId === VERSION
    ).length === callCount && !loading()
  );
  actionHandler = async () => null;

  const slowUpdate = deferred<unknown>();
  const refreshAction = deferred<unknown>();
  actionHandler = async () => refreshAction.promise;
  updateHandler = async () => slowUpdate.promise;
  listHandler = async () => [mod("Action refresh")];
  await freshMods();
  await waitFor(
    () => names().includes("Action refresh") && !loading(),
    "动作后重扫测试"
  );
  button("禁用 Action refresh").click();
  await waitFor(() => button("从文件安装").disabled, "动作进入忙状态");
  refreshAction.resolve(null);
  await waitFor(
    () =>
      !button("禁用 Action refresh").disabled && !button("从文件安装").disabled,
    "动作解除忙状态"
  );
  expect("文件动作不等待联网重扫收尾", !button("从文件安装").disabled);
  slowUpdate.resolve([]);
  actionHandler = async () => null;

  // ── 7. StrictMode 与更新开关 ──
  let onlineCalls = 0;
  settingsHandler = async () => ({ disableModUpdates: true });
  listHandler = async () => [mod("No updates")];
  metaHandler = async () => null;
  updateHandler = async () => {
    onlineCalls += 1;
    return [];
  };
  await freshMods(ROOT, VERSION, true);
  await waitFor(
    () => names().includes("No updates") && !loading(),
    "StrictMode"
  );
  expect(
    "StrictMode 清理后最新扫描仍能收尾",
    names().includes("No updates") && !loading()
  );
  await sleep(120);
  expect("禁止更新设置不发联网检查", onlineCalls === 0);
  settingsHandler = async () => ({});

  // ── 8. 500 条列表滚动 ──
  listHandler = async () =>
    Array.from({ length: 500 }, (_, index) =>
      mod(`Mod ${String(index).padStart(3, "0")}`)
    );
  metaHandler = async () => null;
  updateHandler = async () => [];
  await freshMods();
  await waitFor(
    () => document.querySelectorAll(".vst-mod").length === 500 && !loading(),
    "大列表"
  );
  const scroller = document.querySelector<HTMLElement>(".fixture-wrap")!;
  scroller.scrollTop = scroller.scrollHeight;
  await sleep(60);
  expect(
    "500 条 Mod 列表可滚动到底且无横向溢出",
    scroller.scrollTop > 0 && document.documentElement.scrollWidth <= innerWidth
  );

  // ── 9. 设置页回归保持不变 ──
  render(<GamePanel />);
  await waitFor(
    () => !!document.querySelector('input[aria-label="游戏窗口标题"]'),
    "全局标题控件"
  );
  const titleInput = document.querySelector<HTMLInputElement>(
    'input[aria-label="游戏窗口标题"]'
  )!;
  expect(
    "长说明移出主界面且必要标签保留",
    !document.querySelector(".st-game")?.textContent?.includes("{date}") &&
      !!document.querySelector('input[aria-label="游戏窗口标题"]')
  );
  titleInput.scrollIntoView({ block: "center" });
  await sleep(80);
  titleInput.focus({ preventScroll: true });
  await waitFor(() => !!document.querySelector('[role="tooltip"]'), "键盘说明");
  const tip = document.querySelector<HTMLElement>('[role="tooltip"]')!;
  const rect = tip.getBoundingClientRect();
  expect(
    "Tooltip focus 可达且保留日期时间语法",
    tip.textContent?.includes("{date}") === true &&
      tip.textContent.includes("{time}") &&
      titleInput
        .closest(".st-row")
        ?.querySelector(".st-row__control")
        ?.getAttribute("aria-describedby") === tip.id
  );
  expect(
    "900×600 Tooltip 不出视口",
    rect.left >= 0 &&
      rect.right <= innerWidth &&
      rect.top >= 0 &&
      rect.bottom <= innerHeight
  );

  // 键盘回退定位：无指针坐标时挂在触发元素附近（top/bottom），不跟随指针。
  expect(
    "键盘 focus-visible 回退触发元素定位（非指针跟随）",
    tip.dataset.side === "top" || tip.dataset.side === "bottom"
  );
  titleInput.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true })
  );
  await waitFor(
    () => !document.querySelector('[role="tooltip"]'),
    "Escape 关闭说明"
  );
  expect(
    "Escape 收起 Tooltip 并保留控件焦点",
    document.activeElement === titleInput
  );

  // ── 9.1 Tooltip 指针右下跟随 + 贴边翻转 + 离开收起 ──
  const tipControl = titleInput
    .closest(".st-row")!
    .querySelector<HTMLElement>(".st-row__control")!;
  const firePointer = (
    type: string,
    x: number,
    y: number,
    related: EventTarget | null = null
  ) =>
    tipControl.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        clientX: x,
        clientY: y,
        pointerType: "mouse",
        relatedTarget: related,
      })
    );
  // 进入（React 由 pointerover 合成 pointerenter）；延迟期内移动指针，打开时取最新坐标。
  firePointer("pointerover", 20, 20);
  await sleep(120);
  firePointer("pointermove", 40, 30);
  await waitFor(
    () => !!document.querySelector('[role="tooltip"]'),
    "指针说明出现"
  );
  await sleep(60);
  const pointerTip = document.querySelector<HTMLElement>('[role="tooltip"]')!;
  const tipGap =
    parseFloat(getComputedStyle(pointerTip).getPropertyValue("--tip-gap")) || 8;
  const tipLeft = () => parseFloat(getComputedStyle(pointerTip).left);
  const tipTop = () => parseFloat(getComputedStyle(pointerTip).top);
  expect(
    "Tooltip 出现在指针右下并取延迟期内最新坐标",
    pointerTip.dataset.side === "pointer" &&
      Math.abs(tipLeft() - (40 + tipGap)) <= 1 &&
      Math.abs(tipTop() - (30 + tipGap)) <= 1
  );
  // 打开期间继续移动：气泡跟随指针（portal 不重建、入场动画不重播）。
  firePointer("pointermove", 90, 70);
  await sleep(60);
  expect(
    "Tooltip 跟随指针移动",
    Math.abs(tipLeft() - (90 + tipGap)) <= 1 &&
      Math.abs(tipTop() - (70 + tipGap)) <= 1
  );
  // 贴右下边缘：按轴翻转到左上且不越出视口。
  firePointer("pointermove", innerWidth - 12, innerHeight - 12);
  await sleep(60);
  const flipRect = pointerTip.getBoundingClientRect();
  expect(
    "Tooltip 贴边翻转左上且不越界",
    (pointerTip.dataset.flip ?? "").includes("x") &&
      (pointerTip.dataset.flip ?? "").includes("y") &&
      flipRect.left >= 0 &&
      flipRect.right <= innerWidth &&
      flipRect.top >= 0 &&
      flipRect.bottom <= innerHeight
  );
  // 指针离开：收起。
  firePointer("pointerout", 0, 0, document.body);
  await waitFor(
    () => !document.querySelector('[role="tooltip"]'),
    "指针离开收起"
  );

  // ── 9.2 内存条尾圆角 + 过渡（reduced-motion 同步）──
  const allocatedSegment = document.querySelector<HTMLElement>(
    ".st-game-memory__allocated"
  );
  expect("内存条渲染游戏分配段", !!allocatedSegment);
  if (allocatedSegment) {
    const allocStyle = getComputedStyle(allocatedSegment);
    const pillRadius =
      getComputedStyle(document.documentElement)
        .getPropertyValue("--radius-pill")
        .trim() || "999px";
    expect(
      "游戏分配段尾部胶囊圆角（左端直角接已使用段）",
      allocStyle.borderTopLeftRadius === "0px" &&
        allocStyle.borderBottomLeftRadius === "0px" &&
        allocStyle.borderTopRightRadius === pillRadius &&
        allocStyle.borderBottomRightRadius === pillRadius
    );
    const memoryReduced = matchMedia(
      "(prefers-reduced-motion: reduce)"
    ).matches;
    expect(
      "内存条宽度与圆角过渡尊重 reduced-motion",
      memoryReduced
        ? allocStyle.transitionProperty === "none"
        : allocStyle.transitionProperty.includes("width") &&
            allocStyle.transitionProperty.includes("border-radius")
    );
  }

  titleSupported = false;
  render(null);
  await sleep(30);
  render(<GamePanel />);
  await waitFor(
    () => !!document.querySelector('input[aria-label="游戏窗口标题"]:disabled'),
    "不支持标题平台"
  );
  expect(
    "平台能力条件仍在主界面",
    document
      .querySelector(".st-game")
      ?.textContent?.includes("当前系统不支持此项") === true
  );
  titleSupported = true;
  render(<SettingsPanel root={ROOT} versionId={VERSION} javas={[]} />);
  await waitFor(
    () =>
      !!document.querySelector('.vst-panel input[aria-label="游戏窗口标题"]'),
    "实例标题控件"
  );
  const settingRow = (label: string) =>
    Array.from(document.querySelectorAll<HTMLElement>(".vst-row")).find(
      (row) =>
        row.querySelector(".vst-row__label")?.textContent?.trim() === label
    ) ?? null;
  const integrityRow = settingRow("关闭文件校验");
  const isolationRow = settingRow("版本隔离");
  expect(
    "实例说明与全局一致且危险说明不折叠",
    !!document.querySelector('.vst-panel input[aria-label="自定义信息"]') &&
      integrityRow
        ?.querySelector(".vst-row__hint")
        ?.textContent?.includes("启动时跳过文件完整性校验") === true &&
      isolationRow
        ?.querySelector(".vst-row__hint")
        ?.textContent?.includes("开启后使用独立的存档") === true
  );
}

async function visual(kind: string) {
  document.documentElement.dataset.visual = kind;
  if (kind === "game") {
    render(<GamePanel />);
    await waitFor(
      () => !!document.querySelector('input[aria-label="游戏窗口标题"]'),
      "游戏设置"
    );
  } else if (kind === "settings") {
    render(<SettingsPanel root={ROOT} versionId={VERSION} javas={[]} />);
    await waitFor(
      () =>
        !!document.querySelector('.vst-panel input[aria-label="游戏窗口标题"]'),
      "实例设置"
    );
  } else {
    if (kind === "loading") {
      listHandler = async () => new Promise(() => {});
    } else if (kind === "failure") {
      listHandler = async () => {
        throw new Error("无法读取 Mod 文件夹，请检查目录权限");
      };
    } else if (kind === "icons") {
      listHandler = async () =>
        Array.from({ length: 10 }, (_, index) =>
          mod(`Mod ${String(index).padStart(3, "0")}`, ROOT, {
            enabled: index % 5 !== 0,
          })
        );
      metaHandler = async (args) => {
        const index = Number(String(args.path).match(/Mod (\d{3})/)?.[1] ?? 0);
        return modMeta(`Mod ${String(index).padStart(3, "0")}`, {
          iconPath: "assets/icon.png",
        });
      };
      iconHandler = async () => ICON_DATA_URL;
    } else if (kind === "placeholder") {
      // 首屏占位：元数据一直不返回，用于截图确认文件名/简介/透明占位图。
      listHandler = async () =>
        Array.from({ length: 10 }, (_, index) =>
          mod(`Mod ${String(index).padStart(3, "0")}`, ROOT, {
            enabled: index % 5 !== 0,
          })
        );
      metaHandler = async () => new Promise(() => {});
    } else {
      listHandler = async () =>
        Array.from({ length: 500 }, (_, index) =>
          mod(`Mod ${String(index).padStart(3, "0")}`, ROOT, {
            enabled: index % 5 !== 0,
          })
        );
      metaHandler = async () => null;
    }
    setVersionFocus({ root: ROOT, versionId: VERSION });
    render(<VersionSettings />);
    await waitFor(
      () => !!document.querySelector('[aria-label="版本页分区"]'),
      "版本设置页面"
    );
    button("Mod 管理").click();
    await waitFor(() => !!document.querySelector(".vst-mods"), "Mod 页");
    if (kind !== "loading") await waitFor(() => !loading(), "Mod 结果");
    if (kind === "icons") {
      await waitFor(
        () =>
          document.querySelectorAll('.vst-mod__text[data-meta="true"]')
            .length === 10,
        "图标元数据"
      );
      // 懒加载依赖 IntersectionObserver：逐行滚入视口，避免只加载首屏几行。
      for (const row of document.querySelectorAll<HTMLElement>("li.vst-mod")) {
        row.scrollIntoView({ block: "nearest" });
        await sleep(25);
      }
      await sleep(150);
      await waitFor(
        () =>
          document.querySelectorAll(
            ".vst-mod__icon img:not([data-placeholder])"
          ).length >= 4,
        "Mod 图标"
      );
    }
  }
  document.documentElement.dataset.regression = "visual";
}

const visualKind = new URLSearchParams(location.search).get("visual");
void (visualKind ? visual(visualKind) : run())
  .then(() => {
    if (!visualKind) document.documentElement.dataset.regression = "pass";
  })
  .catch((error: unknown) => {
    checks.push({ name: "回归执行", ok: false, detail: String(error) });
    document.documentElement.dataset.regression = "fail";
  })
  .finally(() => {
    document.getElementById("regression-result")!.textContent = checks
      .map(
        (check) => `${check.ok ? "PASS" : "FAIL"} ${check.name} ${check.detail}`
      )
      .join("\n");
  });
