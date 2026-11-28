/**
 * 社区与下载详情浏览器回归入口。
 *
 * 启动：pnpm dev，然后打开 /tests/ui-remaining/regression.html?theme=dark|light
 * 覆盖：结果卡 SVG 兜底、分页单一控件、社区工具栏；详情页加载器×版本分组（pill 筛选、
 * 加载器×版本卡、所选版本置顶、单行版本卡、R4 首卡间距、懒挂载、另存为取消/保存、
 * 整合包命名 Modal）；显示名/筛选纯函数；另存为 IPC 契约；下载页任务管理（终态忽略、
 * 全局统计、阶段卡、失败复制）与气泡水位。
 *
 * 各组用例独立收集结果，单组失败不阻断其他组验证。
 */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/styles/shell.css";
import "../../src/components/modrinth/modrinth.css";
import "../../src/components/community/community.css";
import "../../src/components/community/projectDetail.css";
import { SettingsProvider } from "../../src/lib/settings";
import { ModrinthResultCard } from "../../src/components/modrinth/ModrinthResultCard";
import { CurseforgeResultCard } from "../../src/components/curseforge/CurseforgeResultCard";
import { CommunityPagination } from "../../src/components/community/CommunityPagination";
import { CommunitySearchPage } from "../../src/components/community/CommunitySearchPage";
import { ProjectDetailPage } from "../../src/components/community/ProjectDetailPage";
import DownloadDetail from "../../src/pages/DownloadDetail";
import { DownloadBubble } from "../../src/components/download/DownloadBubble";
import { connectDownloadCenter } from "../../src/components/download/store";
import { saveAsModrinth } from "../../src/components/modrinth/ipc";
import { saveAsCurseforge } from "../../src/components/curseforge/ipc";
import {
  deriveFileDisplayName,
  fileStem,
  formatRelativeTime,
} from "../../src/components/community/versionDisplay";
import {
  ALL_FILTER,
  buildVersionGroups,
  defaultFilterFor,
  deriveVersionPills,
  dropToVersion,
  isFormatFit,
  versionToDrop,
} from "../../src/components/community/versionFilters";
import type {
  CurseforgeHit,
  ModrinthHit,
  ModrinthVersion,
  ProjectVersionPage,
} from "../../src/components/modrinth/types";

type RegressionCheck = {
  name: string;
  ok: boolean;
  detail: string;
};

type RegressionState = {
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
    __uiRemainingRegression: RegressionState;
  }
}

const checks: RegressionCheck[] = [];
const regressionState: RegressionState = { status: "running", checks };
window.__uiRemainingRegression = regressionState;

const callbacks = new Map<number, (payload: unknown) => void>();
let callbackId = 0;

// 另存为 mock 通道：cancel=null（不建任务）/ save=taskId。
let saveAsMode: "cancel" | "save" = "cancel";
const saveAsCalls: Array<{ command: string; args: Record<string, unknown> }> =
  [];

const VERSION_MANIFEST = {
  latest: { release: "1.21.1", snapshot: "25w45a" },
  versions: [
    { id: "1.21.1", type: "release", releaseTime: "2025-08-08T00:00:00Z" },
    { id: "1.20.1", type: "release", releaseTime: "2023-06-12T00:00:00Z" },
  ],
};

const TASK_SNAPSHOTS = [
  {
    taskId: "task-running",
    kind: "resource",
    revision: 3,
    target: "C:/Nickel-fixture/.minecraft/mods/example-mod.jar",
    label: "动态资源包",
    status: "running",
    cancelRequested: false,
    stage: "下载资源文件",
    detail: "正在接收 assets/example-mod.jar",
    percent: 0.42,
    filesDone: 3,
    filesTotal: 12,
    transferredBytes: 1048576,
    completedBytes: 524288,
    totalBytes: null,
    speedBps: 262144,
    remainingMs: 15000,
    startedAt: Date.now() - 12000,
    endedAt: null,
    message: "",
    files: [
      { name: "example-mod.jar", percent: 0.42, done: false },
      { name: "dependencies.jar", percent: 1, done: true },
    ],
    stages: [
      { name: "解析依赖", detail: "4 个依赖" },
      { name: "下载资源文件", detail: "正在接收 assets/example-mod.jar" },
    ],
  },
  {
    taskId: "task-done",
    kind: "resource",
    revision: 4,
    target: "C:/Nickel-fixture/.minecraft/resourcepacks/fixture.zip",
    label: "已完成材质包",
    status: "done",
    cancelRequested: false,
    stage: "完成",
    detail: "已提交到资源目录",
    percent: 1,
    filesDone: 8,
    filesTotal: 8,
    transferredBytes: 8388608,
    completedBytes: 8388608,
    totalBytes: 8388608,
    speedBps: null,
    remainingMs: null,
    startedAt: Date.now() - 42000,
    endedAt: Date.now() - 5000,
    message: "下载完成。",
    files: [{ name: "fixture.zip", percent: 1, done: true }],
    stages: [{ name: "下载资源文件", detail: "1 个文件" }],
  },
  {
    taskId: "task-error",
    kind: "resource",
    revision: 5,
    target: "C:/Nickel-fixture/.minecraft/shaderpacks/fixture-shader.zip",
    label: "失败项目包",
    status: "error",
    cancelRequested: false,
    stage: "校验文件",
    detail: "fixture-shader.zip",
    percent: 0.73,
    filesDone: 2,
    filesTotal: 3,
    transferredBytes: 2097152,
    completedBytes: 1572864,
    totalBytes: null,
    speedBps: null,
    remainingMs: null,
    startedAt: Date.now() - 30000,
    endedAt: Date.now() - 2000,
    message: "下载校验失败，请返回资源重新下载。",
    files: [{ name: "fixture-shader.zip", percent: 0.73, done: false }],
    stages: [{ name: "校验文件", detail: "fixture-shader.zip" }],
  },
];

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};

window.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    if (command === "settings_get") {
      return window.localStorage.getItem("nickel.settings") ?? "{}";
    }
    if (command === "settings_set") return null;
    if (command === "accounts_list") return [];
    if (command === "instances_get") {
      return JSON.stringify({ roots: [], active: "" });
    }
    if (command === "app_tasks_snapshot") return TASK_SNAPSHOTS;
    if (command === "list_remote_versions") {
      return JSON.stringify(VERSION_MANIFEST);
    }
    if (command === "cf_config_status") {
      return JSON.stringify({ configured: false });
    }
    if (command === "modrinth_save_as" || command === "curseforge_save_as") {
      saveAsCalls.push({ command, args });
      return saveAsMode === "save" ? "fixture-save-task" : null;
    }
    if (command === "modrinth_search" || command === "curseforge_search") {
      const modrinth = command === "modrinth_search";
      return JSON.stringify({
        total: 1,
        hits: [
          {
            slug: modrinth ? "fixture-mod" : "fixture-pack",
            title: modrinth ? "工具栏检索项目" : "工具栏检索整合包",
            description: "固定检索结果用于验证工具栏与来源筛选。",
            iconUrl: "",
            downloads: 4321,
            author: "Fixture Author",
            projectType: "mod",
            versions: ["1.21.1"],
            loaders: ["fabric"],
            dateModified: "2026-03-04T00:00:00Z",
            projectId: modrinth ? "" : "98765",
          },
        ],
      });
    }
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return null;
    return null;
  },
  transformCallback: (callback) => {
    const id = ++callbackId;
    callbacks.set(id, callback);
    return id;
  },
  unregisterCallback: (id) => {
    callbacks.delete(id);
  },
  convertFileSrc: (path) => path,
};

// 复制交互确定性：不依赖浏览器剪贴板权限。
let copiedText = "";
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: {
    writeText: async (text: string) => {
      copiedText = text;
    },
  },
});

const sleep = (ms: number) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, ms));

function required<T extends Element>(root: ParentNode, selector: string): T {
  const node = root.querySelector<T>(selector);
  if (!node) throw new Error(`找不到元素：${selector}`);
  return node;
}

function check(name: string, ok: boolean, detail = ""): void {
  checks.push({ name, ok, detail });
}

function expect(name: string, condition: boolean, detail = ""): void {
  check(name, condition, detail);
  if (!condition) throw new Error(`${name}${detail ? `：${detail}` : ""}`);
}

async function waitFor<T>(
  probe: () => T | null | false | undefined,
  label: string,
  timeout = 5000
): Promise<T> {
  const deadline = performance.now() + timeout;
  while (performance.now() < deadline) {
    const value = probe();
    if (value) return value;
    await sleep(20);
  }
  throw new Error(`等待超时：${label}`);
}

function renderResult(): void {
  const result = required<HTMLElement>(document, "#ui-remaining-result");
  const failed = checks.filter((item) => !item.ok);
  regressionState.status = failed.length ? "fail" : "pass";
  result.dataset.status = regressionState.status;
  result.textContent = checks
    .map(
      (item) =>
        `${item.ok ? "PASS" : "FAIL"}  ${item.name}${
          item.detail ? ` · ${item.detail}` : ""
        }`
    )
    .join("\n");
  document.documentElement.dataset.regression = regressionState.status;
}

/** 分段独立容错：一段失败不阻断其余段（下载段依赖 WP-F 合并）。 */
async function section(name: string, run: () => Promise<void> | void) {
  try {
    await run();
  } catch (error) {
    check(
      `${name}·执行异常`,
      false,
      error instanceof Error ? error.message : String(error)
    );
  }
}

const MODRINTH_HIT: ModrinthHit = {
  slug: "fixture-mod",
  title: "无图标项目",
  description: "用于验证结果卡不会退化为标题首字母图标。",
  iconUrl: "",
  downloads: 123456,
  author: "Fixture Author",
  projectType: "mod",
  versions: ["1.21.1"],
  loaders: ["fabric"],
  dateModified: "2026-01-02T03:04:05Z",
  source: "modrinth",
};

const CURSEFORGE_HIT: CurseforgeHit = {
  ...MODRINTH_HIT,
  slug: "fixture-pack",
  title: "无图标整合包",
  projectType: "modpack",
  dateModified: "2026-02-03T04:05:06Z",
  source: "curseforge",
  projectId: "98765",
};

// JEI 类多加载器项目：验证加载器×版本分组、所选版本置顶、单行卡与懒挂载。
const JEI_HIT: ModrinthHit = {
  ...MODRINTH_HIT,
  slug: "jei-fixture",
  title: "JEI Fixture",
  loaders: ["forge", "neoforge", "fabric"],
};

function jeiVersion(
  id: string,
  overrides: Partial<ModrinthVersion> & { filename: string }
): ModrinthVersion {
  const { filename, ...rest } = overrides;
  return {
    id,
    name: filename,
    versionNumber: "1.0.0",
    loaders: ["fabric"],
    gameVersions: ["1.21.1"],
    datePublished: "2026-01-01T00:00:00Z",
    releaseType: "release",
    dependencies: [],
    files: [
      {
        url: `https://example.invalid/${filename}`,
        filename,
        sizeBytes: 7340032,
        primary: true,
        hashSha1: "",
      },
    ],
    ...rest,
  };
}

const JEI_VERSIONS: ModrinthVersion[] = [
  jeiVersion("v-neo-263", {
    filename: "jei-26.3-neoforge-31.17.0.113.jar",
    versionNumber: "31.17.0.113",
    name: "31.17.0.113 for NeoForge 26.3",
    loaders: ["neoforge"],
    gameVersions: ["26.3"],
    datePublished: "2026-10-02T00:00:00Z",
  }),
  jeiVersion("v-forge-1201", {
    filename: "jei-1.20.1-forge-15.2.0.jar",
    versionNumber: "15.2.0",
    loaders: ["forge"],
    gameVersions: ["1.20.1"],
    datePublished: "2025-06-01T00:00:00Z",
  }),
  jeiVersion("v-fabric-1201", {
    filename: "jei-1.20.1-fabric-15.2.0.jar",
    versionNumber: "15.2.0",
    loaders: ["fabric"],
    gameVersions: ["1.20.1"],
    datePublished: "2025-06-02T00:00:00Z",
  }),
  jeiVersion("v-snapshot", {
    filename: "jei-25w45a-fabric-1.0.jar",
    loaders: ["fabric"],
    gameVersions: ["25w45a"],
    releaseType: "beta",
    datePublished: "2025-11-05T00:00:00Z",
  }),
  jeiVersion("v-untagged", {
    filename: "jei-mystery.jar",
    gameVersions: ["Forge"],
    datePublished: "2024-01-01T00:00:00Z",
  }),
  // 同组重名 → 整组回退完整词干。
  jeiVersion("v-dup-1", {
    filename: "alpha-1.21.1-fabric-1.0.jar",
    versionNumber: "1.0",
    loaders: ["fabric"],
    gameVersions: ["1.21.1"],
    datePublished: "2025-09-01T00:00:00Z",
  }),
  jeiVersion("v-dup-2", {
    filename: "alpha-1.21.1-fabric-2.0.jar",
    versionNumber: "2.0",
    loaders: ["fabric"],
    gameVersions: ["1.21.1"],
    datePublished: "2025-09-02T00:00:00Z",
  }),
  // 长列表：懒挂载断言（视口外的行保持占位）。
  ...Array.from({ length: 60 }, (_, index) =>
    jeiVersion(`v-long-${index}`, {
      filename: `longmod-1.21.1-fabric-${index}.jar`,
      versionNumber: `${index}`,
      loaders: ["fabric"],
      gameVersions: ["1.21.1"],
      datePublished: `2025-08-${String((index % 28) + 1).padStart(
        2,
        "0"
      )}T00:00:00Z`,
    })
  ),
];

const JEI_PAGE: ProjectVersionPage = {
  versions: JEI_VERSIONS,
  offset: 0,
  nextOffset: JEI_VERSIONS.length,
  hasMore: false,
  total: JEI_VERSIONS.length,
};

const JEI_INSTANCE = {
  root: "C:/Nickel-fixture/.minecraft",
  versionId: "fixture-instance",
  dir: "C:/Nickel-fixture/.minecraft",
  label: "Fixture 26.3",
  mcVersion: "26.3",
  loader: "neoforge",
};

// 详情页 adapter 桩：记录调用，结果由 saveAsStubResult 控制（null=取消）。
const detailAdapterState = {
  result: null as string | null,
  calls: [] as Array<{ versionId: string; name?: string }>,
};
const JEI_ADAPTER = {
  openProject: async () => undefined,
  fetchVersions: async () => JEI_PAGE,
  install: async (
    _hit: ModrinthHit,
    version: ModrinthVersion,
    name?: string
  ) => {
    detailAdapterState.calls.push({ versionId: version.id, name });
    return detailAdapterState.result;
  },
};

const PACK_HIT: ModrinthHit = {
  ...MODRINTH_HIT,
  slug: "fixture-modpack",
  title: "整合包 Fixture",
  projectType: "modpack",
};
const PACK_VERSIONS: ModrinthVersion[] = [
  jeiVersion("pack-v1", {
    filename: "fixture-pack-1.0.mrpack",
    versionNumber: "1.0",
    gameVersions: ["1.20.1"],
    loaders: [],
  }),
];
const PACK_PAGE: ProjectVersionPage = {
  versions: PACK_VERSIONS,
  offset: 0,
  nextOffset: 1,
  hasMore: false,
  total: 1,
};
const packAdapterState = {
  calls: [] as Array<{ versionId: string; name?: string }>,
};
const PACK_ADAPTER = {
  openProject: async () => undefined,
  fetchVersions: async () => PACK_PAGE,
  install: async (
    _hit: ModrinthHit,
    version: ModrinthVersion,
    name?: string
  ) => {
    packAdapterState.calls.push({ versionId: version.id, name });
    return "fixture-pack-task";
  },
};

function PaginationFixture() {
  const [page, setPage] = useState(2);
  return (
    <CommunityPagination
      page={page}
      hasMore
      onFirst={() => setPage(1)}
      onPrevious={() => setPage((value) => Math.max(1, value - 1))}
      onNext={() => setPage((value) => value + 1)}
    />
  );
}

function runPureChecks(): void {
  // 显示名派生（R5 用户示例）。
  expect(
    "显示名派生：jei 示例去掉版本/加载器词",
    deriveFileDisplayName({
      filename: "jei-26.3-neoforge-31.17.0.113.jar",
      versionNumber: "31.17.0.113",
      gameVersions: ["26.3"],
    }) === "jei"
  );
  expect(
    "显示名派生：多词干保留",
    deriveFileDisplayName({
      filename: "just-enough-items-1.20.1-forge-15.2.jar",
      versionNumber: "15.2",
      gameVersions: ["1.20.1"],
    }) === "just-enough-items"
  );
  expect(
    "显示名派生：下划线分词",
    deriveFileDisplayName({
      filename: "cool_mod-1.20.1-fabric-1.0.jar",
      versionNumber: "1.0",
      gameVersions: ["1.20.1"],
    }) === "cool-mod"
  );
  expect(
    "显示名派生：空结果回退完整词干",
    deriveFileDisplayName({
      filename: "1.20.1-forge-15.2.jar",
      versionNumber: "15.2",
      gameVersions: ["1.20.1"],
    }) === fileStem("1.20.1-forge-15.2.jar")
  );
  expect(
    "相对时间：解析失败返回空串",
    formatRelativeTime("not-a-date") === "" &&
      formatRelativeTime(new Date(Date.now() - 3 * 86400000).toISOString()) ===
        "3 天前"
  );

  // Drop 换算与格式判定（1.20 → 200，26.1 → 261）。
  expect(
    "Drop 换算兼容 1.x 与 26.x",
    versionToDrop("1.20.1") === 200 &&
      versionToDrop("26.1") === 261 &&
      dropToVersion(200) === "1.20" &&
      dropToVersion(261) === "26.1" &&
      versionToDrop("25w45a") === null
  );
  expect(
    "版本格式判定：^1.\\d 或 ≥26.x",
    isFormatFit("1.20.1") &&
      isFormatFit("26.3") &&
      !isFormatFit("25w45a") &&
      !isFormatFit("b1.7.3")
  );

  // ≥9 自动降级：小版本 → Drop 聚合。
  const many = deriveVersionPills([
    "1.20.1",
    "1.20.2",
    "1.20.3",
    "1.20.4",
    "1.20.5",
    "1.20.6",
    "1.21.1",
    "1.21.2",
    "1.21.3",
    "1.21.4",
  ]);
  expect(
    "pill ≥9 降级为 Drop 聚合",
    many.groupedByDrop &&
      many.pills.length === 2 &&
      many.pills[0] === "1.21" &&
      many.pills[1] === "1.20",
    many.pills.join(",")
  );
  // 仍 ≥9 → 折叠 <1.20 为远古版。
  const manyMajors = deriveVersionPills([
    "1.7.10",
    "1.8.9",
    "1.9.4",
    "1.10.2",
    "1.11.2",
    "1.12.2",
    "1.13.2",
    "1.14.4",
    "1.15.2",
    "1.16.5",
    "1.17.1",
    "1.18.2",
    "1.19.4",
    "1.20.1",
    "1.21.1",
    "26.1",
  ]);
  expect(
    "pill 仍 ≥9 折叠远古版（<1.20）",
    manyMajors.foldOld &&
      !manyMajors.groupedByDrop &&
      manyMajors.pills.includes("远古版") &&
      !manyMajors.pills.includes("1.19.4") &&
      manyMajors.pills.includes("1.20.1") &&
      manyMajors.pills.includes("26.1"),
    manyMajors.pills.join(",")
  );
  const few = deriveVersionPills(["1.20.1", "25w45a", "b1.7.3", "Forge"]);
  expect(
    "pill 少量不降级：快照/远古/其他剔除",
    !few.groupedByDrop &&
      few.pills.includes("1.20.1") &&
      few.pills.includes("快照版") &&
      few.pills.includes("远古版") &&
      !few.pills.includes("Forge"),
    few.pills.join(",")
  );
  expect(
    "默认筛选跟随实例（有精确支持才选组）",
    defaultFilterFor(few, "1.20.1", true) === "1.20.1" &&
      defaultFilterFor(few, "1.20.1", false) === ALL_FILTER &&
      defaultFilterFor(few, "", false) === ALL_FILTER
  );

  // 分组：多加载器拆分 + 所选版本置顶 + 其他版本卡。
  const groups = buildVersionGroups({
    versions: JEI_VERSIONS,
    kind: "mod",
    projectLoaders: JEI_HIT.loaders,
    filter: ALL_FILTER,
    groupedByDrop: false,
    foldOld: false,
    instanceVersion: "26.3",
    instanceLoader: "neoforge",
  });
  const titles = groups.map((group) => group.title);
  expect(
    "分组：所选版本卡置顶且取代同名普通卡",
    titles[0] === "NeoForge 26.3（所选版本）" &&
      groups[0].selected &&
      !titles.includes("NeoForge 26.3"),
    titles.join(",")
  );
  expect(
    "分组：加载器×版本卡与快照/其他卡",
    titles.includes("Forge 1.20.1") &&
      titles.includes("Fabric 1.20.1") &&
      titles.includes("快照版") &&
      titles.includes("其他版本"),
    titles.join(",")
  );
  const fabric121 = groups.find((group) => group.title === "Fabric 1.21.1");
  expect(
    "分组：一个文件多版本进多卡且卡内去重",
    fabric121 !== undefined &&
      fabric121.items.some((item) => item.id === "v-dup-1") &&
      new Set(fabric121.items.map((item) => item.id)).size ===
        fabric121.items.length
  );
  const fabricOnly = JEI_VERSIONS.filter(
    (version) => version.loaders.length === 1 && version.loaders[0] === "fabric"
  );
  const singleLoader = buildVersionGroups({
    versions: fabricOnly,
    kind: "mod",
    projectLoaders: ["fabric"],
    filter: ALL_FILTER,
    groupedByDrop: false,
    foldOld: false,
    instanceVersion: "",
    instanceLoader: "",
  });
  expect(
    "分组：单加载器文件集不拆加载器前缀",
    singleLoader.every((group) => !group.hasLoaderPrefix) &&
      singleLoader.some((group) => group.title === "1.20.1"),
    singleLoader.map((group) => group.title).join(",")
  );
  // CurseForge 命中常缺项目 loaders：拆分依据必须含文件实际加载器。
  const cfLike = buildVersionGroups({
    versions: JEI_VERSIONS,
    kind: "mod",
    projectLoaders: [],
    filter: ALL_FILTER,
    groupedByDrop: false,
    foldOld: false,
    instanceVersion: "",
    instanceLoader: "",
  });
  expect(
    "分组：项目 loaders 缺失仍按文件加载器拆分",
    cfLike.some((group) => group.title === "Forge 1.20.1") &&
      cfLike.some((group) => group.title === "Fabric 1.20.1") &&
      cfLike.some((group) => group.hasLoaderPrefix),
    cfLike.map((group) => group.title).join(",")
  );
  const sameVersion = buildVersionGroups({
    versions: [
      jeiVersion("ord-fabric", {
        filename: "m-1.20.1-fabric-1.jar",
        loaders: ["fabric"],
        gameVersions: ["1.20.1"],
      }),
      jeiVersion("ord-neo", {
        filename: "m-1.20.1-neoforge-1.jar",
        loaders: ["neoforge"],
        gameVersions: ["1.20.1"],
      }),
      jeiVersion("ord-forge", {
        filename: "m-1.20.1-forge-1.jar",
        loaders: ["forge"],
        gameVersions: ["1.20.1"],
      }),
    ],
    kind: "mod",
    projectLoaders: ["forge", "neoforge", "fabric"],
    filter: ALL_FILTER,
    groupedByDrop: false,
    foldOld: false,
    instanceVersion: "",
    instanceLoader: "",
  });
  expect(
    "分组：同版本内 Forge → NeoForge → Fabric",
    sameVersion.map((group) => group.title).join(",") ===
      "Forge 1.20.1,NeoForge 1.20.1,Fabric 1.20.1",
    sameVersion.map((group) => group.title).join(",")
  );
  // 大版本聚合 pill 下仍按精确版本×加载器分卡，不揉进同一版本号卡。
  const dropSplit = buildVersionGroups({
    versions: [
      jeiVersion("drop-forge", {
        filename: "m-1.20.1-forge-1.jar",
        loaders: ["forge"],
        gameVersions: ["1.20.1"],
      }),
      jeiVersion("drop-fabric-4", {
        filename: "m-1.20.4-fabric-1.jar",
        loaders: ["fabric"],
        gameVersions: ["1.20.4"],
      }),
      jeiVersion("drop-fabric-1", {
        filename: "m-1.20.1-fabric-1.jar",
        loaders: ["fabric"],
        gameVersions: ["1.20.1"],
      }),
    ],
    kind: "mod",
    projectLoaders: ["forge", "fabric"],
    filter: "1.20",
    groupedByDrop: true,
    foldOld: false,
    instanceVersion: "",
    instanceLoader: "",
  });
  expect(
    "分组：大版本聚合仍按精确版本×加载器分卡",
    dropSplit.map((group) => group.title).join(",") ===
      "Fabric 1.20.4,Forge 1.20.1,Fabric 1.20.1",
    dropSplit.map((group) => group.title).join(",")
  );
}

async function runResultAndToolbarChecks(): Promise<void> {
  const resultIcons = Array.from(
    document.querySelectorAll<HTMLElement>(".mr-card__icon-fallback")
  );
  expect(
    "结果卡兜底使用 SVG",
    resultIcons.length >= 2 &&
      resultIcons.every(
        (icon) =>
          icon.tagName.toLowerCase() === "svg" && !icon.textContent?.trim()
      ),
    resultIcons.map((icon) => icon.tagName.toLowerCase()).join(", ")
  );

  const pagination = required<HTMLElement>(
    document,
    "[data-fixture='pagination']"
  );
  const pagerButtons = Array.from(
    pagination.querySelectorAll<HTMLButtonElement>("button")
  );
  expect(
    "分页包含返回第一页、上一页与下一页",
    pagerButtons.length === 3 &&
      pagerButtons[0].getAttribute("aria-label") === "返回第一页",
    pagerButtons.map((button) => button.getAttribute("aria-label")).join(", ")
  );
  required<HTMLButtonElement>(
    pagination,
    "button[aria-label='上一页']"
  ).click();
  await waitFor(
    () => pagination.querySelector(".cv-pager__page")?.textContent === "1",
    "上一页按钮更新页码"
  );
  expect(
    "上一页按钮更新页码",
    pagination.querySelector(".cv-pager__page")?.textContent === "1"
  );

  const communitySearch = required<HTMLElement>(
    document,
    "[data-fixture='community-search']"
  );
  const toolbar = await waitFor(
    () => communitySearch.querySelector<HTMLElement>(".cv-toolbar"),
    "社区检索工具栏"
  );
  const sourceTrigger = required<HTMLButtonElement>(
    toolbar,
    ".cv-toolbar__source .ni-select__trigger"
  );
  const loaderTrigger = required<HTMLButtonElement>(
    toolbar,
    ".cv-loader .ni-select__trigger"
  );
  expect(
    "来源与加载器默认文案能区分维度",
    sourceTrigger.textContent?.includes("全部来源") === true &&
      loaderTrigger.textContent?.includes("全部加载器") === true,
    `${sourceTrigger.textContent} / ${loaderTrigger.textContent}`
  );
  expect(
    "检索工具栏不产生横向溢出",
    toolbar.scrollWidth <= toolbar.clientWidth + 1,
    `${toolbar.scrollWidth}/${toolbar.clientWidth}`
  );
  if (window.innerWidth <= 720) {
    const search = required<HTMLElement>(toolbar, ".cv-toolbar__search");
    expect(
      "窄窗搜索框独占一行",
      search.getBoundingClientRect().width > toolbar.clientWidth * 0.9,
      `${search.getBoundingClientRect().width}/${toolbar.clientWidth}`
    );
  }
  sourceTrigger.focus();
  sourceTrigger.click();
  const sourceMenu = await waitFor(
    () => document.querySelector<HTMLElement>(".ni-select__menu"),
    "来源菜单"
  );
  expect(
    "来源菜单保留全部来源并支持键盘焦点",
    Array.from(sourceMenu.querySelectorAll("[role='option']")).some(
      (option) => option.textContent?.trim() === "全部来源"
    ) && document.activeElement === sourceTrigger
  );
  sourceTrigger.dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Escape",
      code: "Escape",
      bubbles: true,
    })
  );
  await waitFor(
    () => !document.querySelector(".ni-select__menu"),
    "来源菜单关闭"
  );
}

async function runDetailChecks(): Promise<void> {
  const project = required<HTMLElement>(
    document,
    "[data-fixture='project-detail']"
  );
  // 懒挂载依赖视口相交：先把详情区滚入视口。
  project.scrollIntoView({ block: "start" });
  await waitFor(() => project.querySelector(".pd-group"), "详情版本分组");

  // R6 两处移除：安装到文案与详情页加载器筛选。
  expect(
    "详情页不再显示「安装到」与加载器筛选",
    project.querySelector(".pd-card__target") === null &&
      project.querySelector(".pd-filters") === null &&
      project.querySelector(".cv-loader") === null
  );

  // pill 行：首位全部、默认跟随实例 26.3。
  const pills = await waitFor(
    () => project.querySelector<HTMLElement>(".pd-pills"),
    "版本筛选 pill 行"
  );
  const pillButtons = Array.from(
    pills.querySelectorAll<HTMLButtonElement>(".pd-pill")
  );
  expect(
    "pill 首位全部且默认选中实例所在组",
    pillButtons[0]?.textContent === ALL_FILTER &&
      pillButtons.some(
        (button) =>
          button.textContent === "26.3" &&
          button.getAttribute("aria-pressed") === "true"
      ),
    pillButtons.map((button) => button.textContent).join(",")
  );

  // 所选版本卡置顶且默认展开；其余默认收起。
  let groups = Array.from(project.querySelectorAll<HTMLElement>(".pd-group"));
  const selected = groups[0];
  await waitFor(
    () =>
      required<HTMLButtonElement>(selected, ".pd-group__toggle").getAttribute(
        "aria-expanded"
      ) === "true",
    "所选版本卡默认展开"
  );
  expect(
    "所选版本卡置顶且默认展开",
    selected.querySelector(".pd-group__title")?.textContent ===
      "NeoForge 26.3（所选版本）",
    selected.querySelector(".pd-group__title")?.textContent ?? ""
  );
  expect(
    "其余分组默认收起",
    groups
      .slice(1)
      .every(
        (group) =>
          required<HTMLButtonElement>(group, ".pd-group__toggle").getAttribute(
            "aria-expanded"
          ) === "false"
      )
  );

  // 单行版本卡：显示名 + 版本号 + R4 间距。
  const selectedList = required<HTMLElement>(selected, ".pd-list");
  expect(
    "R4：组内容区顶部间距 = 卡间距（8px）",
    getComputedStyle(selectedList).paddingTop === "8px",
    getComputedStyle(selectedList).paddingTop
  );
  const row = await waitFor(
    () => selected.querySelector<HTMLButtonElement>(".pd-row"),
    "所选版本卡版本行"
  );
  expect(
    "单行卡显示名取文件名派生（jei）+ 版本号",
    required<HTMLElement>(row, ".pd-row__name").textContent === "jei" &&
      required<HTMLElement>(row, ".pd-row__number").textContent ===
        "31.17.0.113",
    `${required<HTMLElement>(row, ".pd-row__name").textContent} / ${
      required<HTMLElement>(row, ".pd-row__number").textContent
    }`
  );
  const rowHeight = row.getBoundingClientRect().height;
  expect(
    "单行版本卡行高为 42px",
    rowHeight >= 40 && rowHeight <= 46,
    `${rowHeight}`
  );
  expect(
    "窄宽版本行不产生横向溢出",
    row.scrollWidth <= row.clientWidth + 1,
    `${row.scrollWidth}/${row.clientWidth}`
  );
  expect(
    "版本行 aria-label 表达另存为动作",
    row.getAttribute("aria-label")?.includes("另存为 jei") === true,
    row.getAttribute("aria-label") ?? ""
  );
  expect(
    "所选版本卡标题已含加载器 → 行内不重复加载器 chip",
    row.querySelector(".pd-row__loader") === null
  );
  expect(
    "详情不再提供往期加载提示与按钮",
    project.querySelector(".pd-history") === null &&
      !Array.from(project.querySelectorAll("button")).some((node) =>
        node.textContent?.includes("加载往期版本")
      ),
    project.querySelector(".pd-history")?.textContent ?? ""
  );
  const releaseIcon = required<HTMLImageElement>(row, ".pd-row__release");
  const releaseIconWidth = releaseIcon.getBoundingClientRect().width;
  expect(
    "发布通道图标为 Nickel 立方 PNG（32px）",
    releaseIcon.tagName.toLowerCase() === "img" &&
      releaseIconWidth === 32 &&
      (releaseIcon.getAttribute("src") ?? "") !== "",
    `${releaseIcon.tagName}/${releaseIconWidth}`
  );

  // 默认筛选跟随实例（26.3）时只显示匹配置顶卡；切「全部」展开完整分组。
  pillButtons[0].click();
  await waitFor(
    () => project.querySelectorAll(".pd-group").length > 1,
    "全部筛选恢复完整分组"
  );
  groups = Array.from(project.querySelectorAll<HTMLElement>(".pd-group"));
  expect(
    "全部筛选下其余分组默认收起",
    groups
      .filter((group) => group !== selected)
      .every(
        (group) =>
          required<HTMLButtonElement>(group, ".pd-group__toggle").getAttribute(
            "aria-expanded"
          ) === "false"
      )
  );
  expect(
    "加载器×版本卡与快照/其他卡齐全",
    ["Forge 1.20.1", "Fabric 1.20.1", "快照版", "其他版本"].every((title) =>
      groups.some(
        (group) =>
          group.querySelector(".pd-group__title")?.textContent === title
      )
    ),
    groups
      .map((group) => group.querySelector(".pd-group__title")?.textContent)
      .join(",")
  );

  // 展开普通加载器卡：chip 出现（标题未含加载器的卡）。
  const forgeGroup = groups.find(
    (group) =>
      group.querySelector(".pd-group__title")?.textContent === "Forge 1.20.1"
  );
  if (!forgeGroup) throw new Error("缺少 Forge 1.20.1 分组");
  required<HTMLButtonElement>(forgeGroup, ".pd-group__toggle").click();
  await waitFor(
    () =>
      required<HTMLButtonElement>(forgeGroup, ".pd-group__toggle").getAttribute(
        "aria-expanded"
      ) === "true",
    "Forge 分组展开"
  );
  const collapse = required<HTMLElement>(forgeGroup, ".ni-collapse");
  await waitFor(
    () => parseFloat(collapse.style.height) > 0,
    "Collapse 目标高度展开"
  );
  expect(
    "分组展开由共享 Collapse 高度过渡接管",
    collapse.classList.contains("ni-collapse") &&
      parseFloat(collapse.style.height) > 0 &&
      required<HTMLButtonElement>(forgeGroup, ".pd-group__toggle").getAttribute(
        "aria-controls"
      ) === collapse.id
  );

  // 懒挂载：长列表分组展开后，视口外的行保持占位。
  const longGroup = groups.find(
    (group) =>
      group.querySelector(".pd-group__title")?.textContent === "Fabric 1.21.1"
  );
  if (!longGroup) throw new Error("缺少 Fabric 1.21.1 分组");
  longGroup.scrollIntoView({ block: "start" });
  required<HTMLButtonElement>(longGroup, ".pd-group__toggle").click();
  const slots = await waitFor(() => {
    const list = longGroup.querySelectorAll<HTMLLIElement>(".pd-row-slot");
    return list.length >= 60 ? list : null;
  }, "长列表占位行");
  await waitFor(
    () => longGroup.querySelector(".pd-row-slot[data-mounted]"),
    "长列表首行懒挂载"
  );
  const mountedCount = longGroup.querySelectorAll(
    ".pd-row-slot[data-mounted]"
  ).length;
  expect(
    "懒挂载：视口外行保持占位",
    mountedCount < slots.length,
    `${mountedCount}/${slots.length}`
  );
  const placeholder = longGroup.querySelector<HTMLLIElement>(
    ".pd-row-slot:not([data-mounted])"
  );
  if (placeholder) {
    const placeholderHeight = placeholder.getBoundingClientRect().height;
    const style = getComputedStyle(placeholder);
    const reservedHeight = parseFloat(
      style.getPropertyValue("--pd-row-height")
    );
    expect(
      "懒挂载占位只保留版本行高度，不再保留前置折叠入口",
      Math.abs(placeholderHeight - reservedHeight) <= 1,
      `${placeholderHeight}`
    );
  }

  // 同组重名 → 整组回退完整词干。
  const dupRows = Array.from(
    longGroup.querySelectorAll<HTMLButtonElement>(".pd-row")
  ).filter((node) =>
    node.querySelector(".pd-row__name")?.textContent?.startsWith("alpha-")
  );
  expect(
    "同组重名回退完整文件名去扩展名",
    dupRows.length >= 2 &&
      dupRows.every(
        (node) =>
          node.querySelector(".pd-row__name")?.textContent ===
            "alpha-1.21.1-fabric-1.0" ||
          node.querySelector(".pd-row__name")?.textContent ===
            "alpha-1.21.1-fabric-2.0"
      ),
    dupRows
      .map((node) => node.querySelector(".pd-row__name")?.textContent)
      .join(",")
  );

  // 另存为交互：取消（null）不显示已开始；保存（taskId）显示已开始。
  detailAdapterState.calls.length = 0;
  detailAdapterState.result = null;
  row.click();
  await waitFor(
    () => detailAdapterState.calls.length === 1,
    "取消路径调用 adapter"
  );
  await sleep(60);
  expect(
    "另存为取消不标记已开始下载",
    !row.textContent?.includes("已开始下载") &&
      row.querySelector(".pd-row__status svg") !== null
  );
  detailAdapterState.result = "fixture-task";
  row.click();
  await waitFor(
    () => row.textContent?.includes("已开始下载"),
    "保存路径标记已开始"
  );
  expect(
    "另存为保存后显示已开始下载",
    detailAdapterState.calls.length === 2 &&
      row.textContent?.includes("已开始下载") === true
  );

  // pill 切换：选「快照版」后只剩快照卡（本地过滤）。
  const snapshotPill = pillButtons.find(
    (button) => button.textContent === "快照版"
  );
  if (!snapshotPill) throw new Error("缺少快照版 pill");
  snapshotPill.click();
  await waitFor(
    () =>
      project.querySelectorAll(".pd-group").length === 1 &&
      project.querySelector(".pd-group__title")?.textContent === "快照版",
    "快照筛选本地过滤"
  );
  expect(
    "pill 切换本地过滤分组",
    project.querySelectorAll(".pd-group").length === 1
  );
  pillButtons[0].click();
  await waitFor(
    () => project.querySelectorAll(".pd-group").length > 1,
    "恢复全部分组"
  );
}

async function runModpackChecks(): Promise<void> {
  const project = required<HTMLElement>(
    document,
    "[data-fixture='modpack-detail']"
  );
  project.scrollIntoView({ block: "start" });
  const row = await waitFor(
    () => project.querySelector<HTMLButtonElement>(".pd-row:not(:disabled)"),
    "整合包版本行"
  );
  packAdapterState.calls.length = 0;
  row.click();
  const form = await waitFor(
    () => document.querySelector<HTMLFormElement>(".pd-name-form"),
    "整合包命名 Modal"
  );
  expect(
    "整合包保持命名 Modal（不弹另存为）",
    form !== null && saveAsCalls.length === 0
  );
  const input = required<HTMLInputElement>(form, ".pd-name-input");
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value"
  )?.set;
  setter?.call(input, "我的整合包");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  const submit = Array.from(
    document.querySelectorAll<HTMLButtonElement>("button")
  ).find((button) => button.textContent?.trim() === "开始安装");
  if (!submit) throw new Error("缺少开始安装按钮");
  submit.click();
  await waitFor(() => packAdapterState.calls.length === 1, "整合包命名后安装");
  expect(
    "整合包安装携带实例名",
    packAdapterState.calls[0]?.name === "我的整合包" &&
      packAdapterState.calls[0]?.versionId === "pack-v1",
    JSON.stringify(packAdapterState.calls[0])
  );
}

async function runSaveAsIpcChecks(): Promise<void> {
  saveAsCalls.length = 0;
  saveAsMode = "cancel";
  const cancelled = await saveAsModrinth({
    kind: "mod",
    slugOrId: "jei-fixture",
    versionId: "v-neo-263",
    root: "C:/game",
    instanceVersionId: "instance-a",
    dir: "C:/game/versions/instance-a",
    source: "official_auto",
  });
  expect(
    "modrinth_save_as 取消返回 null",
    cancelled === null &&
      saveAsCalls[0]?.command === "modrinth_save_as" &&
      saveAsCalls[0]?.args.instanceVersionId === "instance-a" &&
      saveAsCalls[0]?.args.root === "C:/game" &&
      saveAsCalls[0]?.args.dir === "C:/game/versions/instance-a" &&
      saveAsCalls[0]?.args.versionId === "v-neo-263",
    JSON.stringify(saveAsCalls[0])
  );
  saveAsMode = "save";
  const saved = await saveAsCurseforge({
    kind: "resourcepack",
    projectId: "98765",
    fileId: "12345",
    root: "C:/game",
    instanceVersionId: "instance-a",
    dir: "C:/game",
  });
  expect(
    "curseforge_save_as 保存返回 taskId",
    saved === "fixture-save-task" &&
      saveAsCalls[1]?.command === "curseforge_save_as" &&
      saveAsCalls[1]?.args.projectId === "98765" &&
      saveAsCalls[1]?.args.fileId === "12345",
    JSON.stringify(saveAsCalls[1])
  );
}

async function runDownloadChecks(): Promise<void> {
  const download = required<HTMLElement>(
    document,
    "[data-fixture='download-detail']"
  );
  await waitFor(
    () => download.querySelector(".dd-card[data-status='error']"),
    "下载任务快照（阶段任务卡）"
  );

  expect(
    "终态被忽略：无已完成卡片与旧任务列表",
    download.querySelector(".dd-card[data-status='done']") === null &&
      download.querySelector(".dd-task") === null
  );
  expect(
    "下载页无文件明细/目标路径/单任务统计/清除记录",
    download.querySelector(".dd-files") === null &&
      download.querySelector(".dd-target") === null &&
      download.querySelector(".dd-stats") === null &&
      !Array.from(download.querySelectorAll("button")).some(
        (button) => button.textContent?.trim() === "清除记录"
      )
  );
  expect(
    "活动任务页使用独立页头且不重复全局导航",
    download.querySelectorAll(".dd-page__bar").length === 1 &&
      !Array.from(download.querySelectorAll(".dd-page__bar button")).some(
        (button) => button.textContent?.trim().startsWith("返回")
      )
  );
  expect(
    "窄窗布局让任务详情排在任务列表之前",
    getComputedStyle(required<HTMLElement>(download, ".dd-main")).gridArea ===
      "main" &&
      getComputedStyle(required<HTMLElement>(download, ".dd-side")).gridArea ===
        "side"
  );

  // 全局统计：固定 mock 下只有 task-running 活动。
  const overview = required<HTMLElement>(download, ".dd-overview");
  const statValues = Array.from(
    overview.querySelectorAll<HTMLElement>(".dd-stat__value")
  );
  expect(
    "全局统计三项：总进度/速度/剩余文件",
    statValues.length === 3 &&
      statValues[0].textContent?.includes("42.00") === true &&
      statValues[1].textContent?.includes("KB/s") === true &&
      statValues[2].textContent?.trim() === "9",
    statValues.map((node) => node.textContent).join(" / ")
  );

  // 运行卡：一行一个命名阶段，当前阶段整数百分比。
  const runningCard = required<HTMLElement>(
    download,
    ".dd-card[data-status='running']"
  );
  const stages = Array.from(
    runningCard.querySelectorAll<HTMLElement>(".dd-stage")
  );
  expect(
    "运行卡渲染已发生 + 当前阶段（2 行）",
    stages.length === 2,
    `${stages.length}`
  );
  const currentStage = required<HTMLElement>(
    runningCard,
    ".dd-stage[data-current]"
  );
  expect(
    "当前阶段显示整数百分比",
    required<HTMLElement>(
      currentStage,
      ".dd-stage__status"
    ).textContent?.includes("42%") === true,
    required<HTMLElement>(currentStage, ".dd-stage__status").textContent ?? ""
  );
  expect(
    "已完成阶段显示对勾图标",
    stages[0].querySelector(".dd-stage__status svg") !== null
  );
  expect("运行卡有取消按钮", runningCard.querySelector(".dd-cancel") !== null);

  // 失败卡：错误文本可复制，反馈 aria-live。
  const errorCard = required<HTMLElement>(
    download,
    ".dd-card[data-status='error']"
  );
  const failureText = required<HTMLButtonElement>(
    errorCard,
    ".dd-failure__text"
  );
  expect(
    "失败卡显示可执行错误且不使用原生 title",
    failureText.textContent?.includes("请返回资源重新下载") === true &&
      !failureText.hasAttribute("title"),
    failureText.textContent ?? ""
  );
  failureText.dispatchEvent(
    new PointerEvent("pointerover", {
      bubbles: true,
      pointerType: "mouse",
      clientX: 100,
      clientY: 100,
    })
  );
  const copyTip = await waitFor(
    () =>
      Array.from(document.querySelectorAll('[role="tooltip"]')).find(
        (node) => node.textContent === "单击复制错误详情"
      ),
    "复制错误的跟随鼠标提示"
  );
  expect(
    "复制提示使用统一鼠标跟随模式",
    copyTip.getAttribute("data-side") === "pointer"
  );
  failureText.dispatchEvent(
    new PointerEvent("pointerout", {
      bubbles: true,
      pointerType: "mouse",
    })
  );
  const feedback = required<HTMLElement>(errorCard, ".dd-failure__feedback");
  expect(
    "复制反馈通过 aria-live 播报",
    feedback.getAttribute("aria-live") === "polite"
  );
  copiedText = "";
  failureText.click();
  await waitFor(
    () => feedback.textContent?.includes("已复制错误详情"),
    "复制错误反馈"
  );
  expect(
    "点击错误文本复制详情",
    copiedText.includes("请返回资源重新下载"),
    copiedText
  );
  expect(
    "失败卡有手动移除按钮",
    errorCard.querySelector(".dd-cancel") !== null
  );
  required<HTMLButtonElement>(errorCard, ".dd-cancel").click();
  await waitFor(
    () => download.querySelector(".dd-card[data-status='error']") === null,
    "失败卡手动移除"
  );
  await sleep(700);
  expect(
    "移除的失败卡不被快照复活",
    download.querySelector(".dd-card[data-status='error']") === null
  );
}

async function runBubbleChecks(): Promise<void> {
  const bubble = await waitFor(
    () => document.querySelector<HTMLElement>(".dl-bubble"),
    "下载入口气泡"
  );
  const water = await waitFor(
    () => bubble.querySelector<HTMLElement>(".dl-bubble__water"),
    "气泡水位"
  );
  const clip = getComputedStyle(water).clipPath;
  expect(
    "气泡水位随全局进度填充（42% → inset 58%）",
    clip.includes("inset") && clip.includes("58%"),
    clip
  );
  const reducedMotion = window.matchMedia(
    "(prefers-reduced-motion: reduce)"
  ).matches;
  const overlay = required<HTMLElement>(document, ".dl-overlay");
  const ripple = overlay.querySelector<HTMLElement>(".dl-overlay__ripple");
  if (reducedMotion) {
    expect(
      "reduced-motion 关闭涟漪与水位过渡",
      ripple !== null &&
        getComputedStyle(ripple).display === "none" &&
        getComputedStyle(water).transitionDuration === "0s",
      getComputedStyle(water).transitionDuration
    );
  } else {
    check("气泡水位非减弱动效分支", true, "no-preference");
  }
}

async function runRegression(): Promise<void> {
  const reducedMotion = window.matchMedia(
    "(prefers-reduced-motion: reduce)"
  ).matches;

  // React 19 并发提交晚于 setTimeout(0)：先等 fixture 真正渲染。
  await waitFor(
    () => document.querySelector("[data-fixture='project-detail']"),
    "fixture 渲染提交"
  );

  await section("纯函数", runPureChecks);
  await section("结果卡与工具栏", runResultAndToolbarChecks);
  await section("项目详情", runDetailChecks);
  await section("整合包命名", runModpackChecks);
  await section("另存为 IPC", runSaveAsIpcChecks);

  // 下载中心连接（幂等）：下载页与气泡共用同一 store 快照。
  try {
    await connectDownloadCenter();
  } catch (error) {
    check(
      "下载中心连接",
      false,
      error instanceof Error ? error.message : String(error)
    );
  }
  await section("下载页任务管理", runDownloadChecks);
  await section("气泡水位", runBubbleChecks);

  await section("动效降级", () => {
    const project = required<HTMLElement>(
      document,
      "[data-fixture='project-detail'] .pd-page"
    );
    const downloadPage = document.querySelector<HTMLElement>(
      "[data-fixture='download-detail'] .dd-page"
    );
    if (reducedMotion) {
      expect(
        "reduced-motion 关闭详情与下载页动画",
        getComputedStyle(project).animationName === "none" &&
          (downloadPage === null ||
            getComputedStyle(downloadPage).animationName === "none"),
        "reduce"
      );
    } else {
      expect(
        "详情页使用共享进场动画",
        getComputedStyle(project).animationName.includes("ni-fade-up"),
        "no-preference"
      );
      if (downloadPage) {
        const card = downloadPage.querySelector<HTMLElement>(".dd-card");
        expect(
          "下载页与任务卡使用共享进场动画",
          getComputedStyle(downloadPage).animationName.includes("ni-fade-up") &&
            (card === null ||
              getComputedStyle(card).animationName.includes("ni-fade-up")),
          "no-preference"
        );
      }
    }
    check(
      "reduced-motion 分支已执行",
      true,
      reducedMotion ? "reduce" : "no-preference"
    );
  });

  renderResult();
}

function Fixture() {
  const setTheme = (theme: "dark" | "light") => {
    document.documentElement.dataset.theme = theme;
  };

  return (
    <SettingsProvider>
      <main className="fixture">
        <header className="fixture-head">
          <div>
            <h1>社区与下载详情回归</h1>
            <p>加载器×版本分组 · 单行版本卡 · 另存为 · 下载页密度 · 气泡水位</p>
          </div>
          <div className="fixture-controls" aria-label="测试控制">
            <button type="button" onClick={() => setTheme("dark")}>
              暗色
            </button>
            <button type="button" onClick={() => setTheme("light")}>
              亮色
            </button>
          </div>
        </header>

        <div className="fixture-row">
          <section className="fixture-panel">
            <div className="fixture-label">结果卡 SVG 兜底</div>
            <ul className="fixture-result-list">
              <ModrinthResultCard
                hit={MODRINTH_HIT}
                kind="mod"
                onOpen={() => {}}
              />
              <CurseforgeResultCard
                hit={CURSEFORGE_HIT}
                kind="modpack"
                onOpen={() => {}}
              />
            </ul>
          </section>

          <section data-fixture="pagination" className="fixture-panel">
            <div className="fixture-label">分页单一控件</div>
            <PaginationFixture />
          </section>
        </div>

        <section data-fixture="community-search" className="fixture-panel">
          <div className="fixture-label">社区检索工具栏与来源筛选</div>
          <div className="fixture-host fixture-host--narrow">
            <CommunitySearchPage
              kind="mod"
              label="Mod"
              onOpenDetail={() => {}}
            />
          </div>
        </section>

        <section data-fixture="project-detail" className="fixture-panel">
          <div className="fixture-label">
            项目详情：加载器×版本分组 / 单行卡 / 另存为
          </div>
          <div className="fixture-host fixture-host--narrow">
            <ProjectDetailPage
              kind="mod"
              hit={JEI_HIT}
              instance={JEI_INSTANCE}
              adapter={JEI_ADAPTER}
              onBack={() => {}}
            />
          </div>
        </section>

        <section data-fixture="modpack-detail" className="fixture-panel">
          <div className="fixture-label">整合包命名 Modal 不变</div>
          <div className="fixture-host fixture-host--narrow">
            <ProjectDetailPage
              kind="modpack"
              hit={PACK_HIT}
              instance={JEI_INSTANCE}
              adapter={PACK_ADAPTER}
              onBack={() => {}}
            />
          </div>
        </section>

        <section
          data-fixture="download-detail"
          className="fixture-panel fixture-download"
        >
          <div className="fixture-label">
            下载页任务管理：全局统计 / 阶段卡 / 失败复制
          </div>
          <DownloadDetail />
        </section>

        <pre id="ui-remaining-result" data-status="running" aria-live="polite">
          正在运行回归…
        </pre>
      </main>
      <DownloadBubble />
    </SettingsProvider>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("缺少 #root");
createRoot(root).render(<Fixture />);

window.setTimeout(() => {
  void runRegression();
}, 0);
