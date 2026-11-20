/**
 * LoaderPicker 浏览器回归入口。
 *
 * 启动：pnpm dev，然后打开 /tests/loader-picker/regression.html
 * 该页用与 list_loaders 相同的 camelCase 形状注入固定清单，验证：
 * 展开高度、全部构建可见、不可安装状态、五种 provider 状态、真实重试与旧响应隔离，
 * 以及主加载器互斥、OptiFine 精确版本过滤和 Forge/Fabric/NeoForge/LiteLoader 兼容矩阵。
 */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/vanilla/vanilla.css";
import "../../src/components/vanilla/LoaderPicker.css";
import { LoaderPicker } from "../../src/components/vanilla/LoaderPicker";
import type {
  LoaderCapability,
  LoaderCatalog,
  LoaderOption,
  LoaderProvider,
  LoaderProviderState,
  RemoteVersion,
} from "../../src/components/download/types";
import {
  MAIN_LOADER_IDS,
  downloadSelectionError,
  forgeRequirementMatches,
  mainLoaderConflict,
  optifineCompatibility,
  optifineMatchesMinecraft,
  optifineOptionsView,
  resolvedDownloadIdentity,
} from "../../src/components/vanilla/loaderCompatibility";
import { normalizeLoaders } from "../../src/components/download/parse";

type LoaderKey = keyof Pick<
  LoaderCatalog,
  "forge" | "neoforge" | "fabric" | "liteloader" | "optifine"
>;

type RegressionCheck = {
  name: string;
  ok: boolean;
  detail: string;
};

type FixtureOptiFineOption = LoaderOption & {
  artifact: {
    rawMetadata: {
      minecraftId: string;
      forgeRequirement: string | null;
      optifineVersion: string;
    };
  };
};
type CallRecord = {
  mcVersion: string;
  loader: string;
  forceRefresh: boolean;
};

type FixtureState = {
  status: "running" | "pass" | "fail";
  checks: RegressionCheck[];
  calls: CallRecord[];
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
    __loaderPickerRegression: FixtureState;
  }
}

const VERSION_263: RemoteVersion = {
  id: "26.3",
  type: "release",
  releaseTime: "2026-11-10T00:00:00+00:00",
};
const VERSION_265: RemoteVersion = {
  id: "26.5",
  type: "release",
  releaseTime: "2026-11-12T00:00:00+00:00",
};
const VERSION_264: RemoteVersion = {
  id: "26.4",
  type: "release",
  releaseTime: "2026-11-11T00:00:00+00:00",
};
const VERSION_262: RemoteVersion = {
  id: "26.2",
  type: "release",
  releaseTime: "2026-11-09T00:00:00+00:00",
};
const VERSION_1201: RemoteVersion = {
  id: "1.20.1",
  type: "release",
  releaseTime: "2023-06-12T00:00:00+00:00",
};
const VERSION_1143: RemoteVersion = {
  id: "1.14.3",
  type: "release",
  releaseTime: "2019-06-03T00:00:00+00:00",
};

const calls: CallRecord[] = [];
const fixtureState: FixtureState = { status: "running", checks: [], calls };
window.__loaderPickerRegression = fixtureState;

let callbackId = 0;
const callbacks = new Map<number, (payload: unknown) => void>();

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function provider(
  state: LoaderProviderState,
  install: boolean,
  saveInstaller: boolean,
  error = ""
): LoaderProvider {
  return { state, capability: { install, saveInstaller }, error };
}

function buildOptions(
  mcVersion: string,
  loader: LoaderKey,
  count: number,
  capability: (index: number) => LoaderCapability
): LoaderOption[] {
  return Array.from({ length: count }, (_, index) => {
    const build = index + 1;
    const version = `${mcVersion}-${loader}-60.0.${build}`;
    return {
      version,
      label: version,
      releaseTime: "2026-11-10T00:00:00+00:00",
      modifiedTime: "",
      stable: true,
      latest: false,
      recommended: false,
      downloadUrl: `https://example.invalid/${loader}/${version}`,
      changelogUrl: "",
      capability: capability(index),
    };
  });
}

function loaderOption(
  loader: LoaderKey,
  version: string,
  label: string,
  capability: LoaderCapability
): LoaderOption {
  return {
    version,
    label,
    releaseTime: "2026-11-10T00:00:00+00:00",
    modifiedTime: "",
    stable: true,
    latest: false,
    recommended: false,
    downloadUrl: `https://example.invalid/${loader}/${version}`,
    changelogUrl: "",
    capability,
  };
}

function optifineOption(
  mcVersion: string,
  patch: string,
  forgeRequirement: string | null,
  minecraftId = mcVersion
): FixtureOptiFineOption {
  const version = `OptiFine_${minecraftId}_HD_U_${patch}`;
  return {
    ...loaderOption(
      "optifine",
      version,
      `${mcVersion} OptiFine HD_U_${patch}`,
      { install: true, saveInstaller: true }
    ),
    artifact: {
      rawMetadata: {
        minecraftId,
        forgeRequirement,
        optifineVersion: version,
      },
    },
  };
}

function catalogFor(mcVersion: string, forceRefresh: boolean): LoaderCatalog {
  if (mcVersion === VERSION_1201.id) {
    return {
      forge: [
        loaderOption("forge", "1.20.1-47.2.0", "47.2.0", {
          install: true,
          saveInstaller: true,
        }),
      ],
      neoforge: [
        loaderOption("neoforge", "1.20.1-47.1.99", "47.1.99", {
          install: true,
          saveInstaller: true,
        }),
      ],
      fabric: [
        loaderOption("fabric", "0.16.9", "0.16.9", {
          install: true,
          saveInstaller: true,
        }),
      ],
      liteloader: [],
      optifine: [
        optifineOption("1.20.1", "I6", "47.2.0"),
        optifineOption("1.20.1", "I7", "47.3.0"),
        optifineOption("1.20.1", "I8", null),
        optifineOption("1.20.1", "I9", ""),
        optifineOption("1.12.2", "G5", ""),
      ],
      notes: {},
      providers: {
        forge: provider("available", true, true),
        neoforge: provider("available", true, true),
        fabric: provider("available", true, true),
        liteloader: provider("unavailable", false, false),
        optifine: provider("available", true, true),
      },
    };
  }

  if (mcVersion === VERSION_1143.id) {
    return {
      forge: [
        loaderOption("forge", "1.14.3-27.2.0.261", "27.2.0.261", {
          install: true,
          saveInstaller: true,
        }),
      ],
      neoforge: [],
      fabric: [],
      liteloader: [],
      optifine: [optifineOption("1.14.3", "F5", "")],
      notes: {},
      providers: {
        forge: provider("available", true, true),
        neoforge: provider("hidden", false, false),
        fabric: provider("hidden", false, false),
        liteloader: provider("hidden", false, false),
        optifine: provider("available", true, true),
      },
    };
  }

  if (mcVersion === VERSION_265.id) {
    return {
      forge: [],
      neoforge: [],
      fabric: [],
      liteloader: [],
      optifine: [],
      notes: {},
      providers: {
        forge: provider("unavailable", false, false),
        neoforge: provider("unavailable", false, false),
        fabric: provider("unavailable", false, false),
        liteloader: provider("hidden", false, false),
        optifine: provider("unavailable", false, false),
      },
    };
  }

  if (mcVersion === VERSION_264.id) {
    return {
      forge: buildOptions(mcVersion, "forge", 18, () => ({
        install: true,
        saveInstaller: true,
      })),
      neoforge: [],
      fabric: [],
      liteloader: [],
      optifine: [],
      notes: {},
      providers: {
        forge: provider("available", true, true),
        neoforge: provider("hidden", false, false),
        fabric: provider("hidden", false, false),
        liteloader: provider("hidden", false, false),
        optifine: provider("hidden", false, false),
      },
    };
  }

  if (mcVersion === VERSION_262.id) {
    return {
      forge: buildOptions(mcVersion, "forge", 18, () => ({
        install: false,
        saveInstaller: false,
      })),
      neoforge: [],
      fabric: buildOptions(mcVersion, "fabric", 2, () => ({
        install: true,
        saveInstaller: true,
      })),
      liteloader: [],
      optifine: [],
      notes: {},
      providers: {
        forge: provider("unavailable", false, false),
        neoforge: provider("hidden", false, false),
        fabric: provider("available", true, true),
        liteloader: provider("hidden", false, false),
        optifine: provider("unavailable", false, false),
      },
    };
  }

  const forge = buildOptions(mcVersion, "forge", 18, (index) =>
    index === 17
      ? { install: false, saveInstaller: false }
      : { install: true, saveInstaller: true }
  );
  const neoforge = buildOptions(mcVersion, "neoforge", 98, () => ({
    install: false,
    saveInstaller: true,
  }));
  const fabric = buildOptions(mcVersion, "fabric", 255, () => ({
    install: true,
    saveInstaller: true,
  }));
  const optifine = forceRefresh
    ? buildOptions(mcVersion, "optifine", 1, () => ({
        install: true,
        saveInstaller: true,
      }))
    : [];

  return {
    forge,
    neoforge,
    fabric,
    liteloader: [],
    optifine,
    notes: {},
    providers: {
      forge: provider("available", true, true),
      neoforge: provider("manualOnly", false, true),
      fabric: provider("available", true, true),
      liteloader: provider("unavailable", false, false),
      optifine: forceRefresh
        ? provider("available", true, true)
        : provider("failed", false, false, "OptiFine 上游清单请求超时"),
    },
  };
}

function requestDelay(mcVersion: string): number {
  return mcVersion === VERSION_264.id
    ? 900
    : mcVersion === VERSION_262.id ||
      mcVersion === VERSION_1201.id ||
      mcVersion === VERSION_1143.id ||
      mcVersion === VERSION_265.id
    ? 60
    : 500;
}

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};
window.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    if (command === "list_loader_provider") {
      const mcVersion =
        typeof args.mcVersion === "string" ? args.mcVersion : "";
      const loader = typeof args.loader === "string" ? args.loader : "";
      const forceRefresh = args.forceRefresh === true;
      calls.push({ mcVersion, loader, forceRefresh });
      const delay = requestDelay(mcVersion);
      await sleep(delay);
      const catalog = catalogFor(mcVersion, forceRefresh);
      if (!(loader in catalog.providers)) {
        throw new Error(`未知加载器：${loader}`);
      }
      const key = loader as LoaderKey;
      return JSON.stringify({
        loader: key,
        builds: catalog[key],
        provider: catalog.providers[key],
        cached: false,
        elapsedMs: delay,
      });
    }
    if (command === "list_loaders") {
      const mcVersion =
        typeof args.mcVersion === "string" ? args.mcVersion : "";
      const forceRefresh = args.forceRefresh === true;
      calls.push({ mcVersion, loader: "all", forceRefresh });
      const delay = requestDelay(mcVersion);
      await sleep(delay);
      return JSON.stringify(catalogFor(mcVersion, forceRefresh));
    }
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return null;
    if (command === "app_tasks_snapshot") return [];
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

document.body.style.overflow = "auto";
document.documentElement.style.overflow = "auto";
document.documentElement.dataset.regression = "running";

function Fixture() {
  const [version, setVersion] = useState<RemoteVersion>(VERSION_263);

  return (
    <div className="fixture-wrap">
      <div className="fixture-controls" aria-label="回归场景">
        <button
          id="switch-26-4"
          type="button"
          onClick={() => setVersion(VERSION_264)}
        >
          切到 26.4（慢响应）
        </button>
        <button
          id="switch-26-5"
          type="button"
          onClick={() => setVersion(VERSION_265)}
        >
          切到 26.5（空清单）
        </button>
        <button
          id="switch-26-2"
          type="button"
          onClick={() => setVersion(VERSION_262)}
        >
          切到 26.2（快响应）
        </button>
        <button
          id="switch-1-20-1"
          type="button"
          onClick={() => setVersion(VERSION_1201)}
        >
          切到 1.20.1（兼容矩阵）
        </button>
        <button
          id="switch-1-14-3"
          type="button"
          onClick={() => setVersion(VERSION_1143)}
        >
          切到 1.14.3（Forge 禁用窗口）
        </button>
      </div>
      <LoaderPicker
        version={version}
        source="official"
        fileSource="official"
        instanceDir="C:/Nickel-regression"
        onBack={() => {}}
      />
    </div>
  );
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const node = root.querySelector<T>(selector);
  if (!node) throw new Error(`找不到元素：${selector}`);
  return node;
}

function loaderCard(id: LoaderKey): HTMLElement {
  return required<HTMLElement>(document, `.van-loader[data-loader="${id}"]`);
}

function statusText(id: LoaderKey): string {
  return (
    loaderCard(id).querySelector<HTMLElement>(".van-loader__status")
      ?.textContent ?? ""
  ).trim();
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

async function openLoader(
  id: LoaderKey,
  minimumHeight: number
): Promise<HTMLElement> {
  const card = loaderCard(id);
  const head = required<HTMLButtonElement>(card, ".van-loader__head");
  if (head.getAttribute("aria-expanded") !== "true") head.click();
  await waitFor(
    () => head.getAttribute("aria-expanded") === "true",
    `${id} aria-expanded`
  );
  const expansion = required<HTMLElement>(card, ".van-loader__expansion");
  await waitFor(
    () => expansion.getBoundingClientRect().height > minimumHeight,
    `${id} 展开高度`,
    3000
  );
  return card;
}

async function closeForEscape(card: HTMLElement): Promise<void> {
  const pick = required<HTMLButtonElement>(
    card,
    ".van-builds--all .van-build__pick:not(:disabled)"
  );
  const head = required<HTMLButtonElement>(card, ".van-loader__head");
  pick.focus();
  pick.dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Escape",
      code: "Escape",
      bubbles: true,
    })
  );
  await waitFor(
    () => head.getAttribute("aria-expanded") === "false",
    "Escape 收起"
  );
  expect("Escape 后焦点回到加载器标题", document.activeElement === head);
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

async function runRegression(): Promise<void> {
  try {
    await waitFor(
      () => document.body.textContent?.includes("正在加载"),
      "首帧 loading"
    );
    expect("加载中 provider 显示真实 loading", true);

    const compatibility = JSON.parse(
      String(
        await window.__TAURI_INTERNALS__.invoke("list_loaders", {
          mcVersion: VERSION_263.id,
          source: "official",
        })
      )
    ) as LoaderCatalog;
    expect(
      "兼容 list_loaders 返回全量清单",
      compatibility.forge.length === 18 &&
        compatibility.neoforge.length === 98 &&
        compatibility.fabric.length === 255
    );
    const directMetadata = normalizeLoaders({
      optifine: [
        {
          version: "OptiFine_1.8.9_HD_U_M5",
          label: "OptiFine 1.8.9 HD U M5",
          minecraftId: "1.8.9",
          forgeRequirement: "11.15.1.2318",
          installerUrl: "https://example.invalid/installer.jar",
        },
      ],
    });
    expect(
      "旧后端顶层 OptiFine 元数据不丢失",
      directMetadata.optifine[0].artifact?.rawMetadata.forgeRequirement ===
        "11.15.1.2318" &&
        directMetadata.optifine[0].artifact?.rawMetadata.installerUrl ===
          undefined
    );

    for (const left of MAIN_LOADER_IDS) {
      for (const right of MAIN_LOADER_IDS) {
        if (left === right) continue;
        expect(
          `${left} 与 ${right} 主加载器互斥`,
          mainLoaderConflict({ id: left, version: "test" }, right).length > 0
        );
      }
    }
    expect(
      "下载入口拒绝 Forge + NeoForge",
      downloadSelectionError([
        { id: "forge", version: "1.20.1-47.2.0" },
        { id: "neoforge", version: "1.20.1-47.1.99" },
      ]).includes("不能同时安装")
    );
    expect(
      "下载入口拒绝 Forge + OptiFine 共装",
      downloadSelectionError([
        { id: "forge", version: "1.20.1-47.2.0" },
        { id: "optifine", version: "OptiFine_1.20.1_HD_U_I6" },
      ]).includes("无法自动共装")
    );
    expect(
      "单选下载参数只传一个真实加载器",
      resolvedDownloadIdentity({
        id: "optifine",
        version: "OptiFine_1.20.1_HD_U_I6",
      }).loader === "optifine"
    );

    expect(
      "Forge 需求 null 不兼容",
      !forgeRequirementMatches(null, "1.12.2-14.23.5.2859", "1.12.2")
    );
    expect(
      "Forge 需求空串不限",
      forgeRequirementMatches("", "1.12.2-14.23.5.2859", "1.12.2")
    );
    expect(
      "Forge 带点版本按相等匹配",
      forgeRequirementMatches("14.23.5.2859", "1.12.2-14.23.5.2859", "1.12.2")
    );
    expect(
      "Forge 纯 build 按 Revision 相等",
      forgeRequirementMatches("2859", "1.12.2-14.23.5.2859", "1.12.2")
    );
    expect(
      "Forge 相邻 Revision 不匹配",
      !forgeRequirementMatches("2860", "1.12.2-14.23.5.2859", "1.12.2")
    );

    const optifine1201 = optifineOption("1.20.1", "I6", "47.2.0");
    const optifineMismatch = optifineOption("1.20.1", "I7", "47.3.0");
    const optifineNoForge = optifineOption("1.20.1", "I8", null);
    const optifineUnlimited = optifineOption("1.20.1", "I9", "");
    const optifine1122 = optifineOption("1.12.2", "G5", "");
    const forge1201 = { id: "forge" as const, version: "1.20.1-47.2.0" };
    expect(
      "OptiFine 精确过滤其他 MC 版本",
      !optifineMatchesMinecraft(optifine1122, "1.20.1")
    );
    expect(
      "Forge 匹配需求时仅手动另存",
      optifineCompatibility(optifine1201, "1.20.1", forge1201).state ===
        "manualOnly"
    );
    expect(
      "Forge 需求不一致时不可兼容",
      optifineCompatibility(optifineMismatch, "1.20.1", forge1201).state ===
        "incompatible"
    );
    expect(
      "Forge N/A 时不可兼容",
      optifineCompatibility(optifineNoForge, "1.20.1", forge1201).state ===
        "incompatible"
    );
    expect(
      "Forge 空需求允许另存共装",
      optifineCompatibility(optifineUnlimited, "1.20.1", forge1201).state ===
        "manualOnly"
    );
    const forgeView = optifineOptionsView(
      [
        optifine1201,
        optifineMismatch,
        optifineNoForge,
        optifineUnlimited,
        optifine1122,
      ],
      "1.20.1",
      forge1201
    );
    expect(
      "OptiFine 视图按 Forge 需求与 MC 精确过滤",
      forgeView.state === "manualOnly" &&
        forgeView.options.length === 2 &&
        forgeView.options.every((option) =>
          ["OptiFine_1.20.1_HD_U_I6", "OptiFine_1.20.1_HD_U_I9"].includes(
            option.version
          )
        )
    );
    const noRequirementMetadata = loaderOption(
      "optifine",
      "OptiFine_1.20.1_HD_U_I10",
      "1.20.1 OptiFine HD_U_I10",
      { install: true, saveInstaller: true }
    );
    expect(
      "缺少 Forge 需求元数据时保守另存",
      optifineCompatibility(noRequirementMetadata, "1.20.1", forge1201)
        .state === "manualOnly"
    );
    const neoForgeView = optifineOptionsView([optifine1201], "1.20.1", {
      id: "neoforge",
      version: "1.20.1-47.1.99",
    });
    expect(
      "NeoForge 组合无兼容项时视图为 incompatible",
      neoForgeView.state === "incompatible" && neoForgeView.options.length === 0
    );
    const fabricNewView = optifineOptionsView(
      [optifineOption("1.20.5", "J1", "")],
      "1.20.5",
      { id: "fabric", version: "0.16.9" }
    );
    expect(
      "Fabric 1.20.5+ 视图为 incompatible",
      fabricNewView.state === "incompatible" &&
        fabricNewView.options.length === 0
    );
    expect(
      "1.13.2 Forge + OptiFine 禁用",
      optifineCompatibility(optifineOption("1.13.2", "E6", ""), "1.13.2", {
        id: "forge",
        version: "1.13.2-25.0.219",
      }).state === "incompatible"
    );
    expect(
      "1.14.3 Forge + OptiFine 禁用",
      optifineCompatibility(optifineOption("1.14.3", "F5", ""), "1.14.3", {
        id: "forge",
        version: "1.14.3-27.2.0.261",
      }).state === "incompatible"
    );
    expect(
      "1.14.4 Forge + OptiFine 恢复兼容",
      optifineCompatibility(optifineOption("1.14.4", "F5", ""), "1.14.4", {
        id: "forge",
        version: "1.14.4-28.2.0",
      }).state === "manualOnly"
    );
    expect(
      "NeoForge + OptiFine 全局禁用",
      optifineCompatibility(optifine1201, "1.20.1", {
        id: "neoforge",
        version: "1.20.1-47.1.99",
      }).state === "incompatible"
    );
    expect(
      "Fabric 1.20.5+ 禁用 OptiFine",
      optifineCompatibility(optifineOption("1.20.5", "J1", ""), "1.20.5", {
        id: "fabric",
        version: "0.16.9",
      }).state === "incompatible"
    );
    const fabricOrigins = optifineCompatibility(
      optifineOption("1.15.2", "G1", ""),
      "1.15.2",
      { id: "fabric", version: "0.16.9" }
    );
    expect(
      "Fabric 1.14–1.15 仅提示 OptiFabric Origins",
      fabricOrigins.state === "manualOnly" &&
        fabricOrigins.reason.includes("OptiFabric Origins")
    );
    const fabricDependencies = optifineCompatibility(optifine1201, "1.20.1", {
      id: "fabric",
      version: "0.16.9",
    });
    expect(
      "OptiFabric 与 Fabric API 分开说明",
      fabricDependencies.reason.includes("OptiFabric") &&
        fabricDependencies.reason.includes("Fabric API")
    );
    const liteCompanion = optifineCompatibility(optifine1122, "1.12.2", {
      id: "liteloader",
      version: "1.12.2-SNAPSHOT-r0",
    });
    expect(
      "LiteLoader + OptiFine 使用 mods/{mc}",
      liteCompanion.state === "manualOnly" &&
        liteCompanion.directory === "mods/1.12.2"
    );

    await waitFor(
      () => statusText("forge").includes("18 个版本"),
      "Forge 清单到达"
    );
    expect("Forge 构建数与后端一致", statusText("forge") === "18 个版本");

    expect(
      "本地粗筛不为 26.3 请求 LiteLoader",
      !calls.some(
        (call) =>
          call.mcVersion === VERSION_263.id && call.loader === "liteloader"
      )
    );
    expect(
      "26.3 不渲染不适用的 LiteLoader",
      document.querySelector('.van-loader[data-loader="liteloader"]') === null
    );

    const forgeCard = await openLoader("forge", 180);
    const forgeRows = forgeCard.querySelectorAll(".van-builds--all .van-build");
    expect(
      "Forge 展开显示全部 18 行",
      forgeRows.length === 18,
      `${forgeRows.length}`
    );
    expect(
      "不可安装 Forge 构建仍可见",
      !!forgeCard.querySelector('[data-version="26.3-forge-60.0.18"]')
    );
    const unavailableForge = required<HTMLElement>(
      forgeCard,
      '[data-version="26.3-forge-60.0.18"]'
    );
    expect(
      "不可安装行标记真实能力",
      unavailableForge.dataset.installable === "false" &&
        unavailableForge.textContent?.includes("暂不可自动安装") === true
    );
    expect(
      "不可安装行不能伪装为可选中",
      required<HTMLButtonElement>(unavailableForge, ".van-build__pick").disabled
    );
    expect(
      "Forge 展开高度不再被 CSS 归零",
      required<HTMLElement>(
        forgeCard,
        ".van-loader__expansion"
      ).getBoundingClientRect().height > 180
    );

    if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
      const duration = getComputedStyle(
        required<HTMLElement>(forgeCard, ".van-loader__expansion")
      ).transitionDuration;
      expect(
        "reduced-motion 关闭展开过渡",
        duration.split(",").every((part) => part.trim() === "0s"),
        duration
      );
    } else {
      pushCheck(
        "reduced-motion 分支待单独模拟",
        true,
        "当前系统未启用减弱动效"
      );
    }

    await closeForEscape(forgeCard);

    await waitFor(
      () => statusText("neoforge").includes("仅可另存 · 98 个版本"),
      "NeoForge manualOnly"
    );
    const neoCard = await openLoader("neoforge", 180);
    const neoRows = neoCard.querySelectorAll(".van-builds--all .van-build");
    expect(
      "NeoForge 点击可展开 98 行",
      neoRows.length === 98,
      `${neoRows.length}`
    );
    expect(
      "manualOnly 行可查看但不可自动安装",
      required<HTMLButtonElement>(neoCard, ".van-builds--all .van-build__pick")
        .disabled
    );
    expect(
      "manualOnly 行保留另存入口",
      neoCard.querySelectorAll(
        ".van-builds--all .van-build__actions .ni-iconbtn"
      ).length === 98
    );

    await waitFor(
      () => statusText("fabric").includes("255 个版本"),
      "Fabric 清单"
    );
    const fabricCard = await openLoader("fabric", 180);
    const fabricRows = fabricCard.querySelectorAll(
      ".van-builds--all .van-build"
    );
    expect(
      "Fabric 点击可展开 255 行",
      fabricRows.length === 255,
      `${fabricRows.length}`
    );
    await waitFor(
      () =>
        required<HTMLElement>(
          neoCard,
          ".van-loader__expansion"
        ).getBoundingClientRect().height < 2,
      "NeoForge 在 Fabric 展开时收起"
    );
    pushCheck("NeoForge 在 Fabric 展开时收起", true);
    const fabricList = required<HTMLElement>(fabricCard, ".van-builds--all");
    expect(
      "长列表在滚动容器内可达",
      getComputedStyle(fabricList).overflowY === "auto" &&
        fabricList.scrollHeight > fabricList.clientHeight
    );

    await waitFor(
      () => statusText("optifine") === "加载失败",
      "OptiFine failed"
    );
    const optifineCard = loaderCard("optifine");
    const retry = required<HTMLButtonElement>(
      optifineCard,
      ".van-loader__retry"
    );
    const callsBeforeRetry = calls.length;
    expect("OptiFine 失败态提供重试", !retry.disabled, retry.title);

    retry.click();
    await waitFor(
      () => statusText("optifine") === "1 个版本",
      "OptiFine 重试后成功",
      5000
    );
    expect(
      "重试真实发起 forceRefresh 请求",
      calls.some((call) => call.forceRefresh)
    );
    expect(
      "重试产生新的后端调用",
      calls.length > callsBeforeRetry,
      `${callsBeforeRetry} -> ${calls.length}`
    );
    const optifineOpened = await openLoader("optifine", 60);
    expect(
      "OptiFine 重试成功后列表可用",
      optifineOpened.querySelectorAll(".van-builds--all .van-build").length ===
        1
    );

    const slow = document.querySelector<HTMLButtonElement>("#switch-26-4");
    const fast = document.querySelector<HTMLButtonElement>("#switch-26-2");
    if (!slow || !fast) throw new Error("缺少版本切换控件");
    slow.click();
    await waitFor(
      () =>
        document
          .querySelector<HTMLElement>(".van-picker__meta")
          ?.textContent?.includes("26.4"),
      "切到 26.4"
    );
    fast.click();
    await waitFor(
      () =>
        document
          .querySelector<HTMLElement>(".van-picker__meta")
          ?.textContent?.includes("26.2"),
      "切到 26.2"
    );
    await waitFor(
      () => statusText("fabric").includes("2 个版本"),
      "26.2 清单到达"
    );
    await waitFor(
      () => statusText("forge").includes("18 个版本 · 暂不可安装"),
      "unavailable 但有构建"
    );
    const unavailableForgeCard = await openLoader("forge", 180);
    const unavailableRows = unavailableForgeCard.querySelectorAll(
      ".van-builds--all .van-build"
    );
    expect(
      "provider 不可用但存在构建时仍可展开",
      unavailableRows.length === 18 &&
        Array.from(unavailableRows).every(
          (row) => row.getAttribute("data-installable") === "false"
        ),
      `${unavailableRows.length}`
    );
    expect(
      "隐藏 provider 不渲染空卡片",
      document.querySelector('.van-loader[data-loader="neoforge"]') === null
    );
    await sleep(1100);
    const meta = document.querySelector<HTMLElement>(".van-picker__meta");
    expect(
      "慢旧响应不得覆盖新选择",
      meta?.textContent?.includes("Minecraft 26.2") === true,
      meta?.textContent?.trim()
    );
    expect(
      "旧 26.4 构建未泄漏到 26.2",
      !document.body.textContent?.includes("26.4-forge-60.0.1")
    );
    expect(
      "26.2 Fabric 仍可展开",
      (await openLoader("fabric", 60)).querySelectorAll(
        ".van-builds--all .van-build"
      ).length === 2
    );

    const compat1201 =
      document.querySelector<HTMLButtonElement>("#switch-1-20-1");
    if (!compat1201) throw new Error("缺少 1.20.1 兼容性场景");
    compat1201.click();
    await waitFor(
      () =>
        document
          .querySelector<HTMLElement>(".van-picker__meta")
          ?.textContent?.includes("Minecraft 1.20.1") === true,
      "切到 1.20.1"
    );
    await waitFor(
      () => statusText("forge") === "1 个版本",
      "1.20.1 Forge 清单"
    );

    const compatForgeCard = await openLoader("forge", 60);
    required<HTMLButtonElement>(
      compatForgeCard,
      '[data-version="1.20.1-47.2.0"] .van-build__pick'
    ).click();
    await waitFor(
      () => statusText("neoforge") === "与已选 Forge 互斥",
      "Forge 选择后 NeoForge 互斥"
    );
    for (const id of ["neoforge", "fabric"] as const) {
      expect(
        `Forge 选择后 ${id} 显示互斥`,
        statusText(id) === "与已选 Forge 互斥"
      );
      expect(
        `Forge 选择后 ${id} 不可展开或勾选`,
        required<HTMLButtonElement>(loaderCard(id), ".van-loader__head")
          .disabled
      );
    }

    await waitFor(
      () => statusText("optifine") === "仅可另存 · 2 个版本",
      "Forge 选择后 OptiFine 改为另存"
    );
    const manualOptifineCard = await openLoader("optifine", 60);
    const manualRows = manualOptifineCard.querySelectorAll(
      ".van-builds--all .van-build"
    );
    expect(
      "Forge 选择后只保留匹配需求与不限定的 OptiFine",
      manualRows.length === 2 &&
        !!manualOptifineCard.querySelector(
          '[data-version="OptiFine_1.20.1_HD_U_I6"]'
        ) &&
        !!manualOptifineCard.querySelector(
          '[data-version="OptiFine_1.20.1_HD_U_I9"]'
        )
    );
    expect(
      "不匹配 Forge 需求或明确 N/A 的 OptiFine 不显示",
      !manualOptifineCard.querySelector(
        '[data-version="OptiFine_1.20.1_HD_U_I7"]'
      ) &&
        !manualOptifineCard.querySelector(
          '[data-version="OptiFine_1.20.1_HD_U_I8"]'
        ) &&
        !manualOptifineCard.querySelector(
          '[data-version="OptiFine_1.12.2_HD_U_G5"]'
        )
    );
    expect(
      "Forge + OptiFine 只能另存不能假勾选",
      Array.from(manualRows).every(
        (row) =>
          row.getAttribute("data-installable") === "false" &&
          row.getAttribute("data-compatibility") === "manualOnly" &&
          required<HTMLButtonElement>(row, ".van-build__pick").disabled
      )
    );
    expect(
      "Forge + OptiFine 保留真实另存入口",
      manualOptifineCard.querySelectorAll(
        ".van-builds--all .van-build__actions .ni-iconbtn"
      ).length === 2
    );

    required<HTMLButtonElement>(
      await openLoader("forge", 60),
      ".van-loader__intro button"
    ).click();
    await waitFor(
      () => statusText("neoforge") === "1 个版本",
      "移除 Forge 后恢复主加载器"
    );
    expect(
      "移除 Forge 后 NeoForge 恢复可选",
      !required<HTMLButtonElement>(loaderCard("neoforge"), ".van-loader__head")
        .disabled
    );

    const neoForgeCard = await openLoader("neoforge", 60);
    required<HTMLButtonElement>(
      neoForgeCard,
      '[data-version="1.20.1-47.1.99"] .van-build__pick'
    ).click();
    await waitFor(
      () => statusText("fabric") === "与已选 NeoForge 互斥",
      "NeoForge 选择后 Fabric 互斥"
    );
    expect(
      "NeoForge 选择后 Forge 也被互斥",
      statusText("forge") === "与已选 NeoForge 互斥"
    );
    expect(
      "NeoForge + OptiFine 明确 incompatible",
      statusText("optifine") === "NeoForge 与 OptiFine 不兼容" &&
        loaderCard("optifine").getAttribute("data-state") === "incompatible"
    );
    expect(
      "NeoForge + OptiFine 不提供可选项",
      required<HTMLButtonElement>(loaderCard("optifine"), ".van-loader__head")
        .disabled
    );

    required<HTMLButtonElement>(
      await openLoader("neoforge", 60),
      ".van-loader__intro button"
    ).click();
    await waitFor(
      () => statusText("fabric") === "1 个版本",
      "移除 NeoForge 后恢复 Fabric"
    );
    const fabricCard1201 = await openLoader("fabric", 60);
    required<HTMLButtonElement>(
      fabricCard1201,
      '[data-version="0.16.9"] .van-build__pick'
    ).click();
    await waitFor(
      () => statusText("forge") === "与已选 Fabric 互斥",
      "Fabric 选择后 Forge 互斥"
    );
    expect(
      "Fabric 选择后 NeoForge 也被互斥",
      statusText("neoforge") === "与已选 Fabric 互斥"
    );
    expect(
      "Fabric + OptiFine 只能另存",
      statusText("optifine") === "仅可另存 · 4 个版本"
    );
    required<HTMLButtonElement>(
      await openLoader("fabric", 60),
      ".van-loader__intro button"
    ).click();
    await waitFor(
      () => statusText("forge") === "1 个版本",
      "移除 Fabric 后恢复 Forge"
    );

    const standaloneOptifineCard = await openLoader("optifine", 60);
    expect(
      "移除 Forge 后当前 MC 的 OptiFine 恢复可选",
      standaloneOptifineCard.querySelectorAll(".van-builds--all .van-build")
        .length === 4 &&
        Array.from(
          standaloneOptifineCard.querySelectorAll<HTMLButtonElement>(
            ".van-builds--all .van-build__pick"
          )
        ).every((pick) => !pick.disabled)
    );
    required<HTMLButtonElement>(
      standaloneOptifineCard,
      '[data-version="OptiFine_1.20.1_HD_U_I6"] .van-build__pick'
    ).click();
    await waitFor(
      () => statusText("forge") === "先移除 OptiFine，再选择主加载器",
      "独立 OptiFine 阻止主加载器"
    );
    expect(
      "独立 OptiFine 选择后 Forge 不可勾选",
      required<HTMLButtonElement>(loaderCard("forge"), ".van-loader__head")
        .disabled
    );

    required<HTMLButtonElement>(
      await openLoader("optifine", 60),
      ".van-loader__intro button"
    ).click();
    await waitFor(
      () => statusText("forge") === "1 个版本",
      "移除 OptiFine 后恢复 Forge"
    );

    const compat1143 =
      document.querySelector<HTMLButtonElement>("#switch-1-14-3");
    if (!compat1143) throw new Error("缺少 1.14.3 兼容性场景");
    compat1143.click();
    await waitFor(
      () =>
        document
          .querySelector<HTMLElement>(".van-picker__meta")
          ?.textContent?.includes("Minecraft 1.14.3") === true,
      "切到 1.14.3"
    );
    await waitFor(
      () => statusText("forge") === "1 个版本",
      "1.14.3 Forge 清单"
    );
    const forge1143Card = await openLoader("forge", 60);
    required<HTMLButtonElement>(
      forge1143Card,
      '[data-version="1.14.3-27.2.0.261"] .van-build__pick'
    ).click();
    await waitFor(
      () =>
        statusText("optifine") ===
        "Minecraft 1.13 至 1.14.3 的 Forge 与 OptiFine 不兼容",
      "1.14.3 Forge + OptiFine 禁用"
    );
    expect(
      "1.14.3 Forge + OptiFine 不提供展开",
      required<HTMLButtonElement>(loaderCard("optifine"), ".van-loader__head")
        .disabled
    );
    expect(
      "1.14.3 Forge + OptiFine provider 标记 incompatible",
      loaderCard("optifine").getAttribute("data-state") === "incompatible"
    );
    expect(
      "1.14.3 选择 Forge 后仍隐藏不适用 NeoForge",
      document.querySelector('.van-loader[data-loader="neoforge"]') === null
    );
    expect(
      "1.14.3 选择 Forge 后仍隐藏不适用 Fabric",
      document.querySelector('.van-loader[data-loader="fabric"]') === null
    );

    const empty265 = document.querySelector<HTMLButtonElement>("#switch-26-5");
    if (!empty265) throw new Error("缺少 26.5 空清单场景");
    empty265.click();
    await waitFor(
      () =>
        document
          .querySelector<HTMLElement>(".van-picker__meta")
          ?.textContent?.includes("Minecraft 26.5") === true,
      "切到 26.5"
    );
    await waitFor(
      () => statusText("forge") === "无可用构建",
      "26.5 空清单到达"
    );
    expect(
      "26.5 空清单不误判 failed",
      statusText("forge") === "无可用构建" &&
        loaderCard("forge").getAttribute("data-state") === "unavailable" &&
        !loaderCard("forge").querySelector(".van-loader__retry")
    );
    expect(
      "26.5 空清单不提供无效展开",
      required<HTMLButtonElement>(loaderCard("forge"), ".van-loader__head")
        .disabled
    );
    expect(
      "26.5 本地粗筛不请求也不渲染 LiteLoader",
      !calls.some(
        (call) =>
          call.mcVersion === VERSION_265.id && call.loader === "liteloader"
      ) &&
        document.querySelector('.van-loader[data-loader="liteloader"]') === null
    );
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

createRoot(document.getElementById("root") as HTMLElement).render(<Fixture />);
if (new URLSearchParams(window.location.search).has("visual")) {
  const result = document.querySelector<HTMLElement>("#regression-result");
  if (result) result.textContent = "视觉检查模式：回归脚本未运行";
  document.documentElement.dataset.regression = "visual";
} else {
  void runRegression();
}
