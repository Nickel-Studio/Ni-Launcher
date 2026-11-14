/** pnpm dev 后打开 /tests/community-version/regression.html；真实页面 + mock IPC。 */
import { createRoot } from "react-dom/client";
import { SettingsProvider } from "../../src/lib/settings";
import { setLaunchInstance } from "../../src/lib/instances";
import Resources from "../../src/pages/Resources";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/styles/shell.css";

interface Check {
  name: string;
  ok: boolean;
  detail: string;
}
interface Call {
  command: string;
  args: Record<string, unknown>;
}
interface Context {
  resourceDir: string;
  displayName: string;
  minecraftVersion?: unknown;
  loader: string;
}
const checks: Check[] = [];
const calls: Call[] = [];
const rootDir = "C:/community-version-fixture";
const target = {
  root: rootDir,
  versionId: "arbitrary-directory",
  loader: "fabric",
  icon: "fabric",
};
const otherTarget = { ...target, versionId: "other-directory" };
const contexts = new Map<string, Context>([
  [
    target.versionId,
    {
      resourceDir: rootDir,
      displayName: "Foo 26.3",
      minecraftVersion: "1.26.3",
      loader: "fabric",
    },
  ],
  [
    otherTarget.versionId,
    {
      resourceDir: rootDir,
      displayName: "我的 26.3 整合包",
      minecraftVersion: "26.3",
      loader: "fabric",
    },
  ],
]);
let contextFailure = false;
let taskId = 0;
// R5 另存为 mock 通道：cancel=null（不建任务）/ save=taskId。
let saveAsMode: "cancel" | "save" = "save";
const callbacks = new Map<number, (payload: unknown) => void>();
let callbackId = 0;
const bridge = window as unknown as {
  __TAURI_INTERNALS__: {
    invoke: (
      command: string,
      args?: Record<string, unknown>
    ) => Promise<unknown>;
    transformCallback: (callback: (payload: unknown) => void) => number;
    unregisterCallback: (id: number) => void;
    convertFileSrc: (path: string) => string;
  };
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
  __communityVersionRegression: {
    status: "running" | "pass" | "fail";
    checks: Check[];
    calls: Call[];
  };
};
const result = {
  status: "running" as "running" | "pass" | "fail",
  checks,
  calls,
};
bridge.__communityVersionRegression = result;
const versions = [
  {
    id: "release-current",
    name: "Current release",
    versionNumber: "5.0",
    gameVersions: ["1.26.3"],
    loaders: ["fabric"],
    datePublished: "2026-10-02T00:00:00Z",
    releaseType: "release",
    files: [
      {
        filename: "current.jar",
        url: "https://example.invalid/current.jar",
        sizeBytes: 100,
        primary: true,
      },
    ],
  },
  {
    id: "release-history",
    name: "Historical release",
    versionNumber: "4.0",
    gameVersions: ["1.20.1"],
    loaders: ["fabric"],
    datePublished: "2025-10-02T00:00:00Z",
    releaseType: "release",
    files: [
      {
        filename: "old.jar",
        url: "https://example.invalid/old.jar",
        sizeBytes: 100,
        primary: true,
      },
    ],
  },
];
bridge.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    calls.push({ command, args });
    if (command === "settings_get") return "{}";
    if (command === "settings_set" || command === "plugin:event|unlisten")
      return null;
    if (command === "accounts_list" || command === "app_tasks_snapshot")
      return [];
    if (command === "plugin:event|listen") return ++callbackId;
    if (command === "instances_get")
      return JSON.stringify({ roots: [rootDir], active: rootDir });
    if (command === "resource_instance_context") {
      if (contextFailure) throw new Error("读取实例失败");
      return contexts.get(String(args.versionId)) ?? null;
    }
    if (command === "list_remote_versions")
      return JSON.stringify({ latest: {}, versions: [] });
    if (command === "modrinth_search" || command === "curseforge_search") {
      const mr = command === "modrinth_search";
      return JSON.stringify({
        total: 1,
        hits: [
          {
            slug: mr ? "test-modrinth" : "test-curseforge",
            projectId: mr ? "" : "12345",
            title: mr ? "Modrinth regression" : "CurseForge regression",
            description: "",
            iconUrl: "",
            downloads: 10,
            author: "Fixture",
            projectType: args.kind,
            versions: ["1.26.3", "1.20.1"],
            loaders: ["fabric"],
            dateModified: "2026-10-02T00:00:00Z",
          },
        ],
      });
    }
    if (command === "modrinth_project_versions")
      return JSON.stringify(versions);
    if (command === "curseforge_project_files")
      return JSON.stringify({
        versions,
        offset: 0,
        nextOffset: versions.length,
        hasMore: false,
        total: versions.length,
      });
    if (command === "modrinth_save_as" || command === "curseforge_save_as")
      return saveAsMode === "cancel" ? null : `fixture-task-${++taskId}`;
    if (command === "modrinth_install" || command === "curseforge_install")
      return `fixture-task-${++taskId}`;
    throw new Error(`未预期的测试命令：${command}`);
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
bridge.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };

function expect(name: string, ok: boolean, detail = "") {
  checks.push({ name, ok, detail });
  if (!ok) throw new Error(`${name}：${detail}`);
}
const sleep = (ms: number) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, ms));
async function waitFor<T>(
  probe: () => T | null | undefined | false,
  name: string
): Promise<T> {
  const end = performance.now() + 5000;
  while (performance.now() < end) {
    const found = probe();
    if (found) return found;
    await sleep(20);
  }
  throw new Error(`等待超时：${name}`);
}
function button(label: string, scope: ParentNode = document) {
  return Array.from(scope.querySelectorAll<HTMLButtonElement>("button")).find(
    (node) => node.textContent?.trim() === label
  );
}
function searchVersion() {
  return document.querySelector<HTMLInputElement>(
    'input[aria-label="MC 版本"]'
  );
}
function lastSearch(command: string) {
  return calls.filter((call) => call.command === command).at(-1)?.args
    .gameVersion;
}
async function selectSource(label: string) {
  const trigger = document.querySelector<HTMLButtonElement>(
    ".cv-toolbar__source .nw-select__trigger"
  );
  if (!trigger) throw new Error("缺少来源选择");
  trigger.click();
  const option = await waitFor(
    () =>
      Array.from(
        document.querySelectorAll<HTMLElement>('[role="option"]')
      ).find((node) => node.textContent?.trim() === label),
    label
  );
  option.click();
}
async function openProject(title: string) {
  const card = await waitFor(
    () =>
      Array.from(
        document.querySelectorAll<HTMLButtonElement>(".mr-card__body")
      ).find((node) => node.textContent?.includes(title)),
    title
  );
  card.click();
  await waitFor(() => document.querySelector(".pd-group"), "详情版本组");
  await waitFor(
    () => document.querySelector(".pd-card__title")?.textContent === title,
    "详情项目标题"
  );
}
async function back() {
  const node = button("返回检索");
  if (!node) throw new Error("缺少返回检索");
  node.click();
  await waitFor(() => !document.querySelector(".pd-page"), "返回检索");
}
/** R5：详情页首个版本行（「所选版本」置顶卡默认展开；懒挂载需视口相交）。 */
async function firstDetailRow(): Promise<HTMLButtonElement> {
  document.querySelector(".pd-page")?.scrollIntoView({ block: "start" });
  return waitFor(
    () =>
      document.querySelector<HTMLButtonElement>(
        ".pd-group .pd-row:not(:disabled)"
      ),
    "详情版本行"
  );
}
function saveAsCalls(command: string) {
  return calls.filter((call) => call.command === command);
}
/** 切「全部」pill 显示完整历史分组（默认筛选跟随实例 mcVersion）。 */
async function showAllGroups() {
  const allPill = Array.from(
    document.querySelectorAll<HTMLButtonElement>(".pd-pill")
  ).find((node) => node.textContent === "全部");
  if (allPill && allPill.getAttribute("aria-pressed") !== "true") {
    allPill.click();
  }
  await waitFor(
    () =>
      Array.from(document.querySelectorAll(".pd-group__title")).some(
        (node) => node.textContent === "1.20.1"
      ),
    "全部筛选显示历史组"
  );
}
async function runRegression() {
  try {
    const mod = await waitFor(() => button("Mod"), "社区 Mod 导航");
    mod.click();
    await waitFor(
      () =>
        searchVersion()?.value === "1.26.3" &&
        lastSearch("modrinth_search") === "1.26.3" &&
        lastSearch("curseforge_search") === "1.26.3",
      "默认真实版本"
    );
    expect("Foo 26.3 的双源搜索传入 1.26.3", true);
    for (const [label, command] of [
      ["Modrinth", "modrinth_search"],
      ["CurseForge", "curseforge_search"],
    ]) {
      const count = calls.filter((call) => call.command === command).length;
      await selectSource(label);
      await waitFor(
        () => calls.filter((call) => call.command === command).length > count,
        `${label} 单源请求`
      );
      expect(
        `${label} 单源搜索只使用真实版本`,
        lastSearch(command) === "1.26.3"
      );
    }
    await selectSource("全部来源");
    await openProject("Modrinth regression");
    expect(
      "详情完整历史不传实例名或 MC 截断参数",
      calls
        .filter((call) => call.command === "modrinth_project_versions")
        .every((call) => call.args.gameVersion === "")
    );
    // R7：默认筛选跟随实例 mcVersion；切「全部」后完整历史分组可见。
    await showAllGroups();
    const titles = Array.from(
      document.querySelectorAll(".pd-group__title")
    ).map((node) => node.textContent);
    expect(
      "详情按真实 gameVersions 分组并保留历史",
      titles.includes("1.26.3") &&
        titles.includes("1.20.1") &&
        !titles.includes("Foo 26.3"),
      titles.join(", ")
    );
    expect(
      "R5：详情页移除「安装到」文案（改为另存为对话框）",
      document.querySelector(".pd-card__target") === null &&
        !document.querySelector(".pd-page")?.textContent?.includes("安装到")
    );

    // R5 另存为：取消=null 不建任务；保存=taskId。
    const row = await firstDetailRow();
    saveAsMode = "cancel";
    const tasksBeforeCancel = taskId;
    row.click();
    await waitFor(
      () => saveAsCalls("modrinth_save_as").length >= 1,
      "另存为 IPC（取消路径）"
    );
    const cancelCall = saveAsCalls("modrinth_save_as").at(-1);
    expect(
      "另存为传实例身份与资源版本",
      cancelCall?.args.kind === "mod" &&
        cancelCall.args.versionId === "release-current" &&
        cancelCall.args.root === rootDir &&
        cancelCall.args.instanceVersionId === target.versionId &&
        cancelCall.args.dir === rootDir,
      JSON.stringify(cancelCall?.args)
    );
    await sleep(80);
    expect("取消另存为不创建任务", taskId === tasksBeforeCancel);
    expect(
      "取消另存为不标记已开始下载",
      !row.textContent?.includes("已开始下载")
    );
    saveAsMode = "save";
    row.click();
    await waitFor(() => taskId > tasksBeforeCancel, "另存为保存创建任务");
    expect(
      "保存另存为后行内显示已开始下载",
      row.textContent?.includes("已开始下载") === true
    );

    // R5：mcVersion 变化不再阻止另存为（默认目录只是起点）。
    contexts.get(target.versionId)!.minecraftVersion = "26.3";
    const savesBeforeVersionChange = saveAsCalls("modrinth_save_as").length;
    row.click();
    await waitFor(
      () => saveAsCalls("modrinth_save_as").length > savesBeforeVersionChange,
      "mcVersion 变化后仍可另存为"
    );
    expect(
      "mcVersion 变化不阻止另存为",
      !document.querySelector(".pd-error")?.textContent?.includes("已变化")
    );
    contexts.get(target.versionId)!.minecraftVersion = "1.26.3";

    // 身份复核保留：资源根变化 → 拒绝并不建任务。
    contexts.get(target.versionId)!.resourceDir = `${rootDir}-changed`;
    const savesBeforeDirChange = saveAsCalls("modrinth_save_as").length;
    const tasksBeforeDirChange = taskId;
    row.click();
    await waitFor(
      () =>
        document
          .querySelector(".pd-error")
          ?.textContent?.includes("当前实例已变化"),
      "资源根变化复核错误"
    );
    expect(
      "资源根变化时不启动另存为",
      saveAsCalls("modrinth_save_as").length === savesBeforeDirChange &&
        taskId === tasksBeforeDirChange
    );
    contexts.get(target.versionId)!.resourceDir = rootDir;
    await back();
    await openProject("CurseForge regression");
    expect(
      "CurseForge 详情也保留完整历史",
      calls
        .filter((call) => call.command === "curseforge_project_files")
        .every((call) => call.args.gameVersion === "")
    );
    const cfRow = await firstDetailRow();
    saveAsMode = "cancel";
    const cfTasksBefore = taskId;
    cfRow.click();
    await waitFor(
      () => saveAsCalls("curseforge_save_as").length >= 1,
      "CurseForge 另存为 IPC"
    );
    const cfCall = saveAsCalls("curseforge_save_as").at(-1);
    expect(
      "CurseForge 另存为传项目/文件 ID 与实例身份",
      cfCall?.args.projectId === "12345" &&
        cfCall.args.fileId === "release-current" &&
        cfCall.args.instanceVersionId === target.versionId &&
        cfCall.args.dir === rootDir,
      JSON.stringify(cfCall?.args)
    );
    await sleep(80);
    expect("CurseForge 取消另存为不创建任务", taskId === cfTasksBefore);
    saveAsMode = "save";
    cfRow.click();
    await waitFor(() => taskId > cfTasksBefore, "CurseForge 另存为保存");
    await back();
    setLaunchInstance(otherTarget);
    await waitFor(
      () =>
        searchVersion()?.value === "26.3" &&
        lastSearch("modrinth_search") === "26.3" &&
        lastSearch("curseforge_search") === "26.3",
      "切换实例默认值"
    );
    expect("实例切换后按 minecraftVersion 原样传 26.3，不补 1. 前缀", true);
    contexts.get(otherTarget.versionId)!.minecraftVersion = undefined;
    setLaunchInstance({ ...otherTarget });
    await waitFor(
      () =>
        searchVersion()?.value === "" &&
        lastSearch("modrinth_search") === "" &&
        lastSearch("curseforge_search") === "",
      "未知版本降级"
    );
    expect("旧响应缺版本字段时清除筛选，不使用显示名或目录名", true);
    contextFailure = true;
    setLaunchInstance({ ...target });
    await sleep(100);
    expect(
      "实例 IPC 失败可继续浏览且默认版本为空",
      searchVersion()?.value === "" &&
        Boolean(document.querySelector(".cv-toolbar"))
    );
    const params = calls.filter(
      (call) =>
        call.command.endsWith("_search") ||
        call.command === "modrinth_project_versions" ||
        call.command === "curseforge_project_files"
    );
    expect(
      "所有 MC API 参数均未出现实例显示名或目录身份",
      params.every(
        (call) =>
          ![
            "Foo 26.3",
            "我的 26.3 整合包",
            target.versionId,
            otherTarget.versionId,
          ].includes(String(call.args.gameVersion))
      )
    );
    expect(
      "R5：普通资源不再直接调用 install 命令",
      !calls.some(
        (call) =>
          call.command === "modrinth_install" ||
          call.command === "curseforge_install"
      )
    );
  } catch (error) {
    checks.push({
      name: "回归执行异常",
      ok: false,
      detail: error instanceof Error ? error.message : String(error),
    });
  } finally {
    result.status = checks.some((check) => !check.ok) ? "fail" : "pass";
    document.documentElement.dataset.regression = result.status;
    const output = document.querySelector<HTMLElement>(
      "#community-version-result"
    );
    if (output) {
      output.dataset.status = result.status;
      output.textContent = checks
        .map(
          (check) =>
            `${check.ok ? "PASS" : "FAIL"} ${check.name} ${check.detail}`
        )
        .join("\n");
    }
  }
}
const host = document.getElementById("root");
if (!host) throw new Error("缺少回归根节点");
setLaunchInstance(target);
createRoot(host).render(
  <SettingsProvider>
    <Resources />
  </SettingsProvider>
);
void runRegression();
