/**
 * UI Foundation 浏览器回归入口。
 *
 * 启动：pnpm dev，然后打开 /tests/ui-foundation/regression.html
 * 覆盖 Pager 切换/滚动复位、Segmented 滑动胶囊与键盘、Modal/Select
 * reduced-motion、Vanilla 搜索空态/Collapse 分组无嵌套滚动、Home 启动错误弹窗。
 */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/styles/shell.css";
import "../../src/components/modrinth/modrinth.css";
import { Button } from "../../src/components/ui/Button";
import { InlineNotice } from "../../src/components/ui/InlineNotice";
import { Modal } from "../../src/components/ui/Modal";
import {
  PagerLayout,
  type PagerItem,
} from "../../src/components/ui/PagerLayout";
import { SegmentedControl } from "../../src/components/ui/SegmentedControl";
import { Select } from "../../src/components/ui/Select";
import { IconAlert } from "../../src/components/resources/icons";
import { VanillaPage } from "../../src/components/vanilla/VanillaPage";
import { Home } from "../../src/pages/Home";
import { SettingsProvider } from "../../src/lib/settings";

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
    __uiFoundationRegression: RegressionState;
  }
}

const checks: RegressionCheck[] = [];
const regressionState: RegressionState = { status: "running", checks };
window.__uiFoundationRegression = regressionState;

const callbacks = new Map<number, (payload: unknown) => void>();
const loaderProviderCalls: string[] = [];
let callbackId = 0;

const VERSION_MANIFEST = {
  latest: { release: "1.21.4", snapshot: "25w45a" },
  versions: [
    {
      id: "1.21.4",
      type: "release",
      releaseTime: "2025-12-09T10:00:00+00:00",
    },
    {
      id: "1.21.1",
      type: "release",
      releaseTime: "2024-08-08T10:00:00+00:00",
    },
    {
      id: "1.20.1",
      type: "release",
      releaseTime: "2023-06-12T10:00:00+00:00",
    },
    {
      id: "25w45a",
      type: "snapshot",
      releaseTime: "2025-11-04T10:00:00+00:00",
    },
    {
      id: "24w14potato",
      type: "snapshot",
      releaseTime: "2024-04-01T10:00:00+00:00",
    },
    {
      id: "1.0",
      type: "old_beta",
      releaseTime: "2011-11-18T10:00:00+00:00",
    },
    {
      id: "b1.7.3",
      type: "old_beta",
      releaseTime: "2011-07-08T10:00:00+00:00",
    },
  ],
};

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};

window.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    if (command === "settings_get") {
      return window.localStorage.getItem("nowhere.settings") ?? "{}";
    }
    if (command === "settings_set") return null;
    if (command === "accounts_list") {
      const raw = window.localStorage.getItem("nowhere.accounts");
      return raw ? JSON.parse(raw) : [];
    }
    if (command === "accounts_set_active" || command === "accounts_upsert") {
      return null;
    }
    if (command === "list_remote_versions") {
      return JSON.stringify(VERSION_MANIFEST);
    }
    if (command === "list_loader_provider") {
      const loader = typeof args.loader === "string" ? args.loader : "";
      loaderProviderCalls.push(loader);
      return JSON.stringify({
        loader,
        builds: [],
        provider: {
          state: "hidden",
          capability: { install: false, saveInstaller: false },
          error: "",
        },
        cached: false,
        elapsedMs: 0,
      });
    }
    if (command === "instances_get") {
      return JSON.stringify({ roots: [], active: "" });
    }
    if (command === "default_minecraft_dir") return "";
    if (command === "skin_fetch") return null;
    if (command === "launch_game") {
      const versionId =
        typeof args.versionId === "string" ? args.versionId : "当前实例";
      throw new Error(
        `${versionId} 缺少可用的 Java 运行时，请在版本设置中选择 Java 后重试。`
      );
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

const sleep = (ms: number) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, ms));

function required<T extends Element>(root: ParentNode, selector: string): T {
  const node = root.querySelector<T>(selector);
  if (!node) throw new Error(`找不到元素：${selector}`);
  return node;
}

function findByText<T extends HTMLElement>(
  root: ParentNode,
  selector: string,
  text: string
): T {
  const node = Array.from(root.querySelectorAll<T>(selector)).find(
    (item) => item.textContent?.trim() === text
  );
  if (!node) throw new Error(`找不到文案为「${text}」的 ${selector}`);
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

function setInputValue(input: HTMLInputElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value"
  )?.set;
  if (!setter) throw new Error("输入框 value setter 不可用");
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function renderResult(): void {
  const result = required<HTMLElement>(document, "#ui-regression-result");
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

async function runRegression(): Promise<void> {
  try {
    await waitFor(
      () => document.querySelector(".van-page__body"),
      "Vanilla 版本清单"
    );

    const reducedMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)"
    ).matches;

    const page = required<HTMLElement>(document, ".nw-pager__page");
    page.scrollTop = 120;
    check(
      "Pager 切换前滚动位置已建立",
      page.scrollTop > 0,
      `${page.scrollTop}`
    );
    const pagerRoot = required<HTMLElement>(document, ".nw-pager");
    findByText<HTMLButtonElement>(
      pagerRoot,
      ".nw-pager__item",
      "预览分区"
    ).click();
    if (!reducedMotion) {
      await waitFor(
        () => pagerRoot.querySelector(".nw-swap__layer--out"),
        "Pager outgoing layer"
      );
    }
    await waitFor(
      () =>
        pagerRoot
          .querySelector(".nw-pager__item[data-active='true']")
          ?.textContent?.includes("预览分区"),
      "Pager active 切换"
    );
    expect(
      "Pager 切换重置 scrollTop",
      page.scrollTop === 0,
      `${page.scrollTop}`
    );
    const incoming = required<HTMLElement>(pagerRoot, ".nw-swap__layer--in");
    const pagerAnimation = getComputedStyle(incoming).animationName;
    expect(
      reducedMotion
        ? "reduced-motion 关闭 Pager 切换动画"
        : "Pager 切换使用 keyed 淡入/位移",
      reducedMotion
        ? pagerAnimation === "none"
        : pagerAnimation.includes("nw-swap-in"),
      pagerAnimation
    );

    const seg = required<HTMLElement>(
      document,
      "[data-fixture='segmented'] .nw-seg"
    );
    const pill = required<HTMLElement>(seg, ".nw-seg__pill");
    await waitFor(() => pill.dataset.ready === "true", "Segmented pill ready");
    const beforeTransform = pill.style.transform;
    const firstTab = required<HTMLButtonElement>(seg, "[role='tab']");
    firstTab.focus();
    firstTab.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "ArrowRight",
        code: "ArrowRight",
        bubbles: true,
      })
    );
    await waitFor(
      () =>
        seg
          .querySelectorAll("[role='tab']")[1]
          ?.getAttribute("aria-selected") === "true",
      "Segmented 键盘切换"
    );
    expect(
      "Segmented 键盘焦点跟随选中项",
      document.activeElement === seg.querySelectorAll("[role='tab']")[1]
    );
    expect(
      "Segmented ARIA 保持 tablist/tab",
      seg.getAttribute("role") === "tablist" &&
        seg.querySelectorAll("[role='tab']").length === 3
    );
    expect(
      "Segmented 胶囊位置随选中项移动",
      pill.style.transform !== beforeTransform,
      `${beforeTransform} -> ${pill.style.transform}`
    );
    const pillTransition = getComputedStyle(pill).transitionDuration;
    expect(
      reducedMotion
        ? "reduced-motion 关闭 Segmented 胶囊过渡"
        : "Segmented 胶囊有滑动过渡",
      reducedMotion
        ? pillTransition.split(",").every((part) => part.trim() === "0s")
        : pillTransition.split(",").some((part) => part.trim() !== "0s"),
      pillTransition
    );

    const openModal = required<HTMLButtonElement>(
      document,
      "#fixture-open-modal"
    );
    openModal.focus();
    openModal.click();
    const dialog = await waitFor(
      () => document.querySelector<HTMLElement>("[role='dialog']"),
      "Modal 打开"
    );
    const modalAnimation = getComputedStyle(dialog).animationName;
    const backdropAnimation = getComputedStyle(
      required<HTMLElement>(document, ".nw-modal__backdrop")
    ).animationName;
    expect(
      reducedMotion ? "reduced-motion 关闭 Modal 动画" : "Modal 有进场动画",
      reducedMotion
        ? modalAnimation === "none" && backdropAnimation === "none"
        : modalAnimation.includes("nw-modal-in") &&
            backdropAnimation.includes("nw-fade"),
      `${modalAnimation} / ${backdropAnimation}`
    );
    expect("Modal 打开后焦点在弹窗内", dialog.contains(document.activeElement));
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        bubbles: true,
      })
    );
    await waitFor(
      () => !document.querySelector("[role='dialog']"),
      "Modal 关闭"
    );
    await waitFor(
      () => document.activeElement === openModal,
      "Modal 焦点归还触发按钮"
    );
    expect("Modal 关闭后焦点归还", document.activeElement === openModal);

    const trigger = required<HTMLButtonElement>(
      document,
      "[data-fixture='overlays'] .nw-select__trigger"
    );
    trigger.click();
    const menu = await waitFor(
      () => document.querySelector<HTMLElement>(".nw-select__menu"),
      "Select 打开"
    );
    const selectAnimation = getComputedStyle(menu).animationName;
    expect(
      reducedMotion ? "reduced-motion 关闭 Select 动画" : "Select 有弹出动画",
      reducedMotion
        ? selectAnimation === "none"
        : selectAnimation.includes("nw-pop"),
      selectAnimation
    );
    expect(
      "Select 打开后选项可见且 ARIA 完整",
      menu.querySelectorAll("[role='option']").length === 3
    );
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        bubbles: true,
      })
    );
    await waitFor(
      () => !document.querySelector(".nw-select__menu"),
      "Select 关闭"
    );
    expect("Select 关闭后焦点回到 trigger", document.activeElement === trigger);

    await waitFor(
      () => loaderProviderCalls.length === 8,
      "页面进入预热最新正式版与预览版"
    );
    const preloadStart = loaderProviderCalls.length;
    const preloadTarget =
      Array.from(
        document.querySelectorAll<HTMLButtonElement>(
          "[data-fixture='vanilla'] .van-version"
        )
      ).find((item) => item.textContent?.includes("1.21.1")) ?? null;
    if (!preloadTarget) throw new Error("找不到 1.21.1 版本行");
    preloadTarget.focus();
    await waitFor(
      () => loaderProviderCalls.length === preloadStart + 4,
      "聚焦版本预热加载器清单"
    );
    const preloadedProviders = new Set(loaderProviderCalls.slice(preloadStart));
    expect(
      "聚焦版本预热四个适用 provider",
      preloadedProviders.size === 4 &&
        ["forge", "neoforge", "fabric", "optifine"].every((id) =>
          preloadedProviders.has(id)
        ) &&
        !preloadedProviders.has("liteloader"),
      [...preloadedProviders].join(", ")
    );

    const search = required<HTMLInputElement>(
      document,
      "[data-fixture='vanilla'] .mr-search input"
    );
    setInputValue(search, "zzzz-no-version");
    const empty = await waitFor(
      () => document.querySelector<HTMLElement>(".van-empty"),
      "Vanilla 搜索空态"
    );
    expect(
      "Vanilla 空态显示筛选词",
      empty.textContent?.includes("zzzz-no-version") === true
    );
    const clear = findByText<HTMLButtonElement>(empty, "button", "清除筛选");
    expect("Vanilla 空态提供清除筛选", !!clear);
    clear.click();
    await waitFor(
      () =>
        !document.querySelector(".van-empty") &&
        document.querySelector(".van-latest"),
      "Vanilla 清除筛选"
    );
    expect(
      "Vanilla 清除后恢复版本列表",
      !!document.querySelector(".van-latest")
    );

    const releaseGroup = required<HTMLElement>(
      document,
      "[data-version-group='release']"
    );
    const groupHead = required<HTMLButtonElement>(
      releaseGroup,
      ".van-group__head"
    );
    expect(
      "Vanilla 正式版分组默认展开",
      groupHead.getAttribute("aria-expanded") === "true"
    );
    groupHead.click();
    await waitFor(
      () => groupHead.getAttribute("aria-expanded") === "false",
      "Vanilla 分组收起"
    );
    const collapse = required<HTMLElement>(
      releaseGroup,
      ".van-group__collapse"
    );
    await waitFor(
      () => parseFloat(getComputedStyle(collapse).height) < 1,
      "Vanilla Collapse 高度归零",
      1500
    );
    const list = required<HTMLElement>(releaseGroup, ".van-group__list");
    const listStyle = getComputedStyle(list);
    expect(
      "Vanilla 分组不再嵌套滚动",
      listStyle.overflowY !== "auto" && listStyle.overflowY !== "scroll",
      listStyle.overflowY
    );
    expect(
      "Vanilla 分组移除 max-height",
      listStyle.maxHeight === "none",
      listStyle.maxHeight
    );
    expect(
      "Vanilla Collapse 使用共享高度机制",
      collapse.classList.contains("nw-collapse")
    );

    const launchButton = required<HTMLButtonElement>(
      document,
      "[data-fixture='home'] .launch-button"
    );
    await waitFor(() => !launchButton.disabled, "Home 启动按钮可用");
    launchButton.click();
    // R7 起启动失败收敛为 LaunchErrorDialog（Modal portal 到 body），不再是行内通知。
    const homeError = await waitFor(
      () =>
        document.querySelector<HTMLElement>(".nw-modal .launch-error__message"),
      "Home 启动错误弹窗"
    );
    expect(
      "Home 启动错误使用弹窗",
      homeError.textContent?.includes("Java 运行时") === true
    );
    expect(
      "Home 弹窗标题为启动失败",
      document
        .querySelector(".nw-modal__title")
        ?.textContent?.includes("启动失败") === true
    );
    expect(
      "Home 不再渲染字符图标",
      !document.querySelector(
        "[data-fixture='home'] .home__launch-error, [data-fixture='home'] .home__launch-warning"
      )
    );
    // 关闭弹窗，归还焦点，避免影响后续断言。
    const dismissButton = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".nw-modal__foot button")
    ).find((button) => button.textContent?.trim() === "知道了");
    dismissButton?.click();

    check(
      "reduced-motion 分支已执行",
      true,
      reducedMotion ? "reduce" : "no-preference"
    );
  } catch (error) {
    check(
      "回归执行异常",
      false,
      error instanceof Error ? error.message : String(error)
    );
  } finally {
    renderResult();
  }
}

const PAGER_ITEMS: PagerItem[] = [
  { id: "alpha", label: "默认分区" },
  { id: "beta", label: "预览分区" },
];

function Fixture() {
  const [pagerPage, setPagerPage] = useState("alpha");
  const [segment, setSegment] = useState("stable");
  const [modalOpen, setModalOpen] = useState(false);
  const [selectValue, setSelectValue] = useState("one");

  const setTheme = (theme: "dark" | "light") => {
    document.documentElement.dataset.theme = theme;
  };

  return (
    <SettingsProvider>
      <main className="fixture">
        <header className="fixture-head">
          <div>
            <h1>UI Foundation 回归</h1>
            <p>Pager · Segmented · Modal/Select · Vanilla · InlineNotice</p>
          </div>
          <div className="fixture-controls" aria-label="测试控制">
            <button
              id="fixture-theme-dark"
              type="button"
              onClick={() => setTheme("dark")}
            >
              暗色
            </button>
            <button
              id="fixture-theme-light"
              type="button"
              onClick={() => setTheme("light")}
            >
              亮色
            </button>
          </div>
        </header>

        <div className="fixture-grid">
          <section
            id="fixture-pager"
            className="fixture-panel fixture-panel--wide"
          >
            <div className="fixture-label">
              Pager keyed crossfade + scroll reset
            </div>
            <div className="fixture-pager-host">
              <PagerLayout
                className="nw-pager--fill"
                items={PAGER_ITEMS}
                active={pagerPage}
                onSelect={setPagerPage}
                navLabel="Pager 回归分区"
                navTitle="Pager"
              >
                <div className="fixture-pager-page">
                  <h2>
                    {pagerPage === "alpha" ? "默认分区内容" : "预览分区内容"}
                  </h2>
                  {Array.from(
                    { length: pagerPage === "alpha" ? 20 : 3 },
                    (_, index) => (
                      <div className="fixture-pager-row" key={index}>
                        {pagerPage === "alpha" ? "长内容" : "短内容"}{" "}
                        {index + 1}
                      </div>
                    )
                  )}
                </div>
              </PagerLayout>
            </div>
          </section>

          <section data-fixture="segmented" className="fixture-panel">
            <div className="fixture-label">
              Segmented sliding indicator + keyboard
            </div>
            <div className="fixture-seg-row">
              <SegmentedControl
                value={segment}
                onChange={setSegment}
                options={[
                  { value: "stable", label: "稳定版" },
                  { value: "preview", label: "预览版" },
                  { value: "legacy", label: "历史版" },
                ]}
              />
              <span>当前：{segment}</span>
            </div>
          </section>

          <section data-fixture="notices" className="fixture-panel">
            <div className="fixture-label">InlineNotice tones</div>
            <div className="fixture-notice-stack">
              <InlineNotice tone="info" icon={<IconAlert />}>
                正在读取远端版本清单。
              </InlineNotice>
              <InlineNotice tone="warning" icon={<IconAlert />}>
                当前网络较慢，将自动切换到备用镜像。
              </InlineNotice>
              <InlineNotice tone="danger" icon={<IconAlert />} title="启动失败">
                当前实例缺少可用的 Java 运行时，请在版本设置中选择 Java 后重试。
              </InlineNotice>
            </div>
          </section>

          <section
            data-fixture="overlays"
            className="fixture-panel fixture-panel--wide"
          >
            <div className="fixture-label">Modal / Select motion + focus</div>
            <div className="fixture-overlay-row">
              <Button
                id="fixture-open-modal"
                type="button"
                variant="secondary"
                onClick={() => setModalOpen(true)}
              >
                打开弹窗
              </Button>
              <Select
                className="fixture-overlay-select"
                ariaLabel="Select 回归选项"
                value={selectValue}
                onChange={setSelectValue}
                options={[
                  { value: "one", label: "第一项" },
                  { value: "two", label: "第二项" },
                  { value: "three", label: "第三项" },
                ]}
              />
            </div>
          </section>

          <section
            data-fixture="vanilla"
            className="fixture-panel fixture-panel--wide"
          >
            <div className="fixture-label">
              Vanilla search empty + shared Collapse
            </div>
            <div className="fixture-vanilla-scroll">
              <VanillaPage />
            </div>
          </section>

          <section
            data-fixture="home"
            className="fixture-panel fixture-panel--wide"
          >
            <div className="fixture-label">Home 启动错误弹窗</div>
            <div className="fixture-home-host">
              <Home />
            </div>
          </section>
        </div>

        <pre id="ui-regression-result" data-status="running" aria-live="polite">
          正在运行回归…
        </pre>

        <Modal
          open={modalOpen}
          onClose={() => setModalOpen(false)}
          title="回归弹窗"
          description="用于验证 reduced-motion、焦点陷阱与关闭后的焦点归还。"
          width={440}
        >
          <label>
            弹窗内输入框
            <input
              aria-label="弹窗内输入框"
              style={{ display: "block", width: "100%", marginTop: 8 }}
            />
          </label>
        </Modal>
      </main>
    </SettingsProvider>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("缺少 #root");
createRoot(root).render(<Fixture />);

window.setTimeout(() => {
  void runRegression();
}, 0);
