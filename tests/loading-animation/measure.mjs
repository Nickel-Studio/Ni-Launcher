/**
 * 用系统 Edge（CDP，不依赖 Playwright）在 900×600 下截图并测量加载动画。
 *
 * 用法（先启动 `pnpm dev -- --port 1425`）：
 *   node tests/loading-animation/measure.mjs --port 1425
 *   node tests/loading-animation/measure.mjs --port 1425 --prefix before- --label before
 *
 * 产物：
 *   design/loading-animation/<prefix><scenario>-<theme>.png
 *   design/loading-animation/measurements-<label>.json
 *
 * 测量口径（两个「中心」都显式定义，避免歧义）：
 *   ink          当前可见挖矿笔画（地面 / 矿块 / 镐子）的屏幕并集包围盒
 *   artSlot      .ni-loading__clip，即卡片分配给动画的内容区（留白裁切框）
 *   cardContent  .ni-loading__card 的 padding 内容框（动画 + 文案的完整内容区）
 *   available    页面为加载态预留的区域（页头以下 / Pager 页面内容区）
 *
 * 判定：
 *   |ink 中心 − artSlot 中心| ≤ 2px      —— 动画本体在留白裁切框内居中
 *   |loader 中心 − available 中心| ≤ 2px —— 页面级加载在页头以下垂直居中
 *   各场景 cardContent 偏移一致（极差 ≤ 1px）—— 组件观感不随页面变化
 *   所有 page 尺寸使用同一 viewBox；900×600 无横向溢出；挥镐不越出裁切框。
 */
import { spawn } from "node:child_process";
import { mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const ROOT = process.cwd();
const OUT_DIR = path.join(ROOT, "design", "loading-animation");
const EDGE =
  process.env.EDGE_PATH ??
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const DEBUG_PORT = 9333;
const PAGE_SCENARIOS = new Set([
  "vanilla",
  "community",
  "settings",
  "game",
  "vsettings",
  "versions",
]);


const args = process.argv.slice(2);
function argValue(name, fallback) {
  const index = args.indexOf(`--${name}`);
  return index >= 0 ? args[index + 1] : fallback;
}
const PORT = Number(argValue("port", "1425"));
const PREFIX = argValue("prefix", "");
const LABEL = argValue("label", "run");
const BASE = `http://localhost:${PORT}`;
const SCENARIOS = (
  argValue(
    "scenarios",
    "vanilla,community,mods,settings,game,vsettings,versions,detail,error"
  ) ?? ""
)
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const THEMES = (argValue("themes", "dark,light") ?? "")
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const STRICT = !args.includes("--no-assert");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitForJson(url, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    try {
      const response = await fetch(url);
      if (response.ok) return await response.json();
    } catch {
      /* Edge 还没起来 */
    }
    await sleep(120);
  }
  throw new Error(`等待 CDP 就绪超时：${url}`);
}

class Cdp {
  constructor(socket, errors = []) {
    this.socket = socket;
    this.errors = errors;
    this.nextId = 0;
    this.pending = new Map();
    socket.onmessage = (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) {
        if (message.method === "Runtime.exceptionThrown") {
          errors.push(
            message.params.exceptionDetails.exception?.description ??
              message.params.exceptionDetails.text
          );
        }
        return;
      }
      const entry = this.pending.get(message.id);
      if (!entry) return;
      this.pending.delete(message.id);
      if (message.error) entry.reject(new Error(message.error.message));
      else entry.resolve(message.result);
    };
    socket.onclose = () => {
      for (const entry of this.pending.values()) {
        entry.reject(new Error("CDP 连接已关闭"));
      }
      this.pending.clear();
    };
  }

  send(method, params = {}) {
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
}

const MEASURE_EXPRESSION = String.raw`(() => {
  // 与 Node 端 PAGE_SCENARIOS 保持一致：只有这些场景断言页面级居中。
  const PAGE_SCENARIOS = new Set([
    "vanilla",
    "community",
    "settings",
    "game",
    "vsettings",
    "versions",
  ]);
  const round = (value) => Math.round(value * 100) / 100;
  const rectOf = (element) => {
    const rect = element.getBoundingClientRect();
    return {
      x: round(rect.x),
      y: round(rect.y),
      width: round(rect.width),
      height: round(rect.height),
      top: round(rect.top),
      right: round(rect.right),
      bottom: round(rect.bottom),
      left: round(rect.left),
      centerX: round((rect.left + rect.right) / 2),
      centerY: round((rect.top + rect.bottom) / 2),
    };
  };
  const contentRectOf = (element) => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    const paddingTop = parseFloat(style.paddingTop) || 0;
    const paddingBottom = parseFloat(style.paddingBottom) || 0;
    const paddingLeft = parseFloat(style.paddingLeft) || 0;
    const paddingRight = parseFloat(style.paddingRight) || 0;
    const borderTop = parseFloat(style.borderTopWidth) || 0;
    const borderBottom = parseFloat(style.borderBottomWidth) || 0;
    const top = rect.top + borderTop + paddingTop;
    const bottom = rect.bottom - borderBottom - paddingBottom;
    return {
      top: round(top),
      bottom: round(bottom),
      left: round(rect.left + paddingLeft),
      right: round(rect.right - paddingRight),
      height: round(bottom - top),
      centerY: round((top + bottom) / 2),
    };
  };
  const showable = (element) => {
    if (!element) return false;
    if (element.closest(".ni-swap__layer--out")) return false;
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const animations = document.getAnimations();
  animations.forEach((animation) => {
    try {
      const timing = animation.effect && animation.effect.getComputedTiming();
      if (timing && timing.iterations === Infinity) {
        animation.pause();
        animation.currentTime = 0;
      } else {
        animation.finish();
      }
    } catch {
      /* 忽略无法暂停的动画 */
    }
  });

  const loaders = Array.from(document.querySelectorAll(".ni-loading"));
  const loader = loaders.find((element) => showable(element)) || loaders[0];
  if (!loader) {
    return {
      ok: false,
      reason: "no .ni-loading",
      scenario: document.documentElement.dataset.scenario,
    };
  }
  const card = loader.querySelector(".ni-loading__card");
  const clip = loader.querySelector(".ni-loading__clip");
  const svg = loader.querySelector(".ni-loading__art");
  const label = loader.querySelector(".ni-loading__label");
  const oreX = loader.querySelector(".ni-loading__ore-x");
  const cardRect = rectOf(card);
  const cardContent = contentRectOf(card);
  const clipRect = rectOf(clip);
  const loaderRect = rectOf(loader);
  const svgRect = rectOf(svg);

  const inkOf = () => {
    const visible = (element) => {
      for (let node = element; node && node !== svg; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (
          style.display === "none" ||
          style.visibility === "hidden" ||
          parseFloat(style.opacity) === 0
        ) {
          return false;
        }
      }
      return true;
    };
    const elements = Array.from(
      svg.querySelectorAll(
        ".ni-loading__ground, .ni-loading__block path, .ni-loading__pick path"
      )
    ).filter(visible);
    let box = null;
    for (const element of elements) {
      const rect = element.getBoundingClientRect();
      if (rect.width === 0 && rect.height === 0) continue;
      if (!box) {
        box = {
          top: rect.top,
          right: rect.right,
          bottom: rect.bottom,
          left: rect.left,
        };
      } else {
        box.top = Math.min(box.top, rect.top);
        box.right = Math.max(box.right, rect.right);
        box.bottom = Math.max(box.bottom, rect.bottom);
        box.left = Math.min(box.left, rect.left);
      }
    }
    if (!box) return null;
    return {
      top: round(box.top),
      right: round(box.right),
      bottom: round(box.bottom),
      left: round(box.left),
      width: round(box.right - box.left),
      height: round(box.bottom - box.top),
      centerY: round((box.top + box.bottom) / 2),
      centerX: round((box.left + box.right) / 2),
    };
  };

  const ink = inkOf();

  // 采样一次完整的挥镐循环，确认裁切框不会切到镐头或碎屑。
  const pick = svg.querySelector(".ni-loading__pick");
  const swing = pick
    ? pick
        .getAnimations()
        .find((animation) => animation.animationName === "ni-loading-swing")
    : null;
  let inkTopMin = ink ? ink.top : null;
  let inkBottomMax = ink ? ink.bottom : null;
  if (swing) {
    const duration =
      (swing.effect && swing.effect.getTiming().duration) || 1800;
    for (let step = 0; step <= 24; step += 1) {
      swing.currentTime = (step / 24) * duration;
      const sample = inkOf();
      if (sample) {
        inkTopMin = Math.min(inkTopMin, sample.top);
        inkBottomMax = Math.max(inkBottomMax, sample.bottom);
      }
    }
    swing.currentTime = 0;
  }

  const bbox = svg.getBBox();
  const ctm = svg.getScreenCTM();
  const point = (x, y) => new DOMPoint(x, y).matrixTransform(ctm);
  const corners = [
    point(bbox.x, bbox.y),
    point(bbox.x + bbox.width, bbox.y),
    point(bbox.x, bbox.y + bbox.height),
    point(bbox.x + bbox.width, bbox.y + bbox.height),
  ];
  const geometricY = corners.map((corner) => corner.y);
  const geometricInk = {
    top: round(Math.min(...geometricY)),
    bottom: round(Math.max(...geometricY)),
    centerY: round((Math.min(...geometricY) + Math.max(...geometricY)) / 2),
  };

  const scenario = document.documentElement.dataset.scenario;
  const page = loader.closest(".van-page--loading") || loader.closest(".mr-page--loading");
  let available = null;
  let pageCentered = false;
  if (page) {
    const head = page.querySelector(".van-page__head, .cv-page__head");
    const pageRect = rectOf(page);
    const gap = parseFloat(getComputedStyle(page).rowGap) || 0;
    const expectedTop = head
      ? head.getBoundingClientRect().bottom + gap
      : pageRect.top;
    const expectedBottom =
      pageRect.bottom - (parseFloat(getComputedStyle(page).paddingBottom) || 0);
    available = {
      source: "页头以下的加载区",
      top: round(expectedTop),
      bottom: round(expectedBottom),
      centerY: round((expectedTop + expectedBottom) / 2),
      gap: round(gap),
      pageMinHeight: getComputedStyle(page).minHeight,
      pageHeight: pageRect.height,
      pagerPageHeight: loader.closest(".ni-pager__page")?.getBoundingClientRect().height ?? null,
    };
    pageCentered = PAGE_SCENARIOS.has(scenario);
  } else if (loader.closest(".vs-root--loading")) {
    // 实例列表：页头与「显示隐藏实例」之后是加载卡片占位区，卡片在该区内居中。
    const main = loader.closest(".vs-main");
    const mainBox = contentRectOf(main);
    const gap = parseFloat(getComputedStyle(main).rowGap) || 0;
    let previous = loader.previousElementSibling;
    while (previous && previous.getBoundingClientRect().height === 0) {
      previous = previous.previousElementSibling;
    }
    const expectedTop = previous
      ? previous.getBoundingClientRect().bottom + gap
      : mainBox.top;
    available = {
      source: "实例列表加载占位区（前一可见块之后）",
      top: round(expectedTop),
      bottom: mainBox.bottom,
      centerY: round((expectedTop + mainBox.bottom) / 2),
      gap: round(gap),
    };
    pageCentered = PAGE_SCENARIOS.has(scenario);
  } else if (loader.classList.contains("ni-loading--viewport")) {
    const area = contentRectOf(loader.closest(".ni-content"));
    available = { source: ".ni-content 内容框", ...area };
    pageCentered = true;
  } else if (loader.closest(".ni-pager__page")) {
    const area = contentRectOf(loader.closest(".ni-pager__page"));
    available = { source: ".ni-pager__page 内容框", ...area };
    pageCentered = PAGE_SCENARIOS.has(scenario);
  } else {
    const area = contentRectOf(loader.parentElement);
    available = {
      source: "父容器内容框",
      parent: loader.parentElement.className,
      ...area,
    };
  }

  const offset = (a, b) =>
    a === null || b === null ? null : round(a - b);

  return {
    ok: true,
    scenario,
    theme: document.documentElement.dataset.theme,
    viewport: {
      width: window.innerWidth,
      height: window.innerHeight,
      dpr: window.devicePixelRatio,
    },
    overflow: {
      documentScrollWidth: document.documentElement.scrollWidth,
      bodyScrollWidth: document.body.scrollWidth,
      innerWidth: window.innerWidth,
      horizontalOverflow:
        document.documentElement.scrollWidth > window.innerWidth ||
        document.body.scrollWidth > window.innerWidth,
    },
    loaderClass: loader.className,
    errorState: loader.classList.contains("ni-loading--error"),
    errorLabel: label?.textContent ?? "",
    errorXOpacity: oreX ? getComputedStyle(oreX).opacity : null,
    loader: loaderRect,
    card: cardRect,
    cardContent,
    artSlot: clipRect,
    label: rectOf(label),
    svg: {
      viewBox: svg.getAttribute("viewBox"),
      attrWidth: svg.getAttribute("width"),
      attrHeight: svg.getAttribute("height"),
      preserveAspectRatio: svg.getAttribute("preserveAspectRatio"),
      rect: svgRect,
      bbox: {
        x: round(bbox.x),
        y: round(bbox.y),
        width: round(bbox.width),
        height: round(bbox.height),
      },
    },
    ink,
    geometricInk,
    clipping: {
      // 正数 = 笔画与裁切框边缘之间的空隙；负数 = 被裁掉。
      inkTopMin: round(inkTopMin),
      inkBottomMax: round(inkBottomMax),
      clearanceTop: round(inkTopMin - clipRect.top),
      clearanceBottom: round(clipRect.bottom - inkBottomMax),
    },
    available,
    pageCentered,
    offsets: {
      inkVsArtSlotCenter: offset(ink && ink.centerY, clipRect.centerY),
      inkVsCardContentCenter: offset(ink && ink.centerY, cardContent.centerY),
      inkVsCardBoxCenter: offset(ink && ink.centerY, cardRect.centerY),
      artSlotVsCardContentCenter: offset(clipRect.centerY, cardContent.centerY),
      cardVsLoaderCenter: offset(cardRect.centerY, loaderRect.centerY),
      loaderVsAvailableCenter: offset(loaderRect.centerY, available.centerY),
      inkVsAvailableCenter: offset(ink && ink.centerY, available.centerY),
    },
    animations: {
      total: animations.length,
      infinite: animations.filter((animation) => {
        const timing = animation.effect && animation.effect.getComputedTiming();
        return timing && timing.iterations === Infinity;
      }).length,
    },
  };
})()`;

function evaluateAssertions(measurements) {
  const failures = [];
  const notes = [];
  for (const item of measurements) {
    const name = `${item.scenario}/${item.theme}`;
    const artOffset = item.offsets.inkVsArtSlotCenter;
    if (Math.abs(artOffset) > 2) {
      failures.push(
        `${name}: 动画绘制内容与留白裁切框中心偏差 ${artOffset}px > 2px`
      );
    }
    if (item.pageCentered) {
      const pageOffset = item.offsets.loaderVsAvailableCenter;
      if (Math.abs(pageOffset) > 2) {
        failures.push(
          `${name}: 加载卡片与页面内容区中心偏差 ${pageOffset}px > 2px`
        );
      }
    }
    if (item.overflow.horizontalOverflow) {
      failures.push(`${name}: 900×600 出现横向溢出`);
    }
    if (item.svg.viewBox !== "0 0 160 114") {
      failures.push(`${name}: viewBox 不是统一的 0 0 160 114（${item.svg.viewBox}）`);
    }
    if (
      item.clipping.clearanceTop < -0.5 ||
      item.clipping.clearanceBottom < -0.5
    ) {
      failures.push(
        `${name}: 挥镐越出裁切框（上 ${item.clipping.clearanceTop}px / 下 ${item.clipping.clearanceBottom}px）`
      );
    }
    if (item.scenario === "error") {
      if (!item.errorState) failures.push(`${name}: 错误场景缺少 .ni-loading--error`);
      if (item.errorLabel !== "网络故障") {
        failures.push(`${name}: 错误文案不是「网络故障」（${item.errorLabel}）`);
      }
      if (Number(item.errorXOpacity) < 0.99) {
        failures.push(`${name}: 错误叉号未定格（opacity ${item.errorXOpacity}）`);
      }
    }
  }

  const pageItems = measurements.filter(
    (item) => item.pageCentered && !item.loaderClass.includes("ni-loading--inline")
  );
  const contentOffsets = pageItems.map(
    (item) => item.offsets.inkVsCardContentCenter
  );
  if (contentOffsets.length > 1) {
    const spread = Math.max(...contentOffsets) - Math.min(...contentOffsets);
    if (spread > 1) {
      failures.push(
        `各页面「动画中心 − 卡片内容框中心」不一致：极差 ${spread}px > 1px`
      );
    } else {
      notes.push(
        `各页面「动画中心 − 卡片内容框中心」一致：${contentOffsets
          .map((value) => `${value}px`)
          .join(" / ")}（动画 + 文案的堆叠结构固定）`
      );
    }
  }
  if (measurements.length > 0) {
    const viewBoxes = new Set(measurements.map((item) => item.svg.viewBox));
    if (viewBoxes.size !== 1) {
      failures.push(`viewBox 不统一：${[...viewBoxes].join(" / ")}`);
    }
  }
  return { failures, notes };
}

/** 等待 fixture 打上 ready（目标场景的加载态稳定渲染）。 */
async function waitForScene(cdp, scenario) {
  const started = Date.now();
  while (Date.now() - started < 30000) {
    const probe = await cdp.send("Runtime.evaluate", {
      expression: `location.search.includes("scenario=${scenario}") && document.documentElement.dataset.ready === "true"`,
      returnByValue: true,
    });
    if (probe.result?.value === true) return;
    await sleep(100);
  }
  const debug = await cdp.send("Runtime.evaluate", {
    expression:
      'JSON.stringify({url:location.href, ready:document.documentElement.dataset.ready, scenario:document.documentElement.dataset.scenario, loaders:document.querySelectorAll(".ni-loading").length, text:document.body.innerText.slice(0,600)})',
    returnByValue: true,
  });
  throw new Error(`场景等待超时：${scenario} ${debug.result?.value ?? ""}`);
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });
  const profile = path.join(os.tmpdir(), `nickel-loading-edge-${Date.now()}`);
  const pageErrors = [];
  /** 减弱动效探针结果（循环动画应为 none / 0 running）。 */
  let reducedMotion = null;
  const edge = spawn(
    EDGE,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-extensions",
      "--hide-crash-restore-bubble",
      `--remote-debugging-port=${DEBUG_PORT}`,
      `--user-data-dir=${profile}`,
      "--window-size=900,600",
      "about:blank",
    ],
    { stdio: "ignore" }
  );

  let socket;
  const measurements = [];
  let runError = null;
  try {
    const targets = await waitForJson(
      `http://127.0.0.1:${DEBUG_PORT}/json/list`,
      20000
    );
    const target = targets.find((item) => item.type === "page");
    if (!target) throw new Error("找不到 Edge 页面 target");
    socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.onopen = resolve;
      socket.onerror = () => reject(new Error("CDP WebSocket 连接失败"));
    });
    const cdp = new Cdp(socket, pageErrors);
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Emulation.setDeviceMetricsOverride", {
      width: 900,
      height: 600,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await cdp.send("Emulation.setEmulatedMedia", {
      media: "screen",
      features: [{ name: "prefers-reduced-motion", value: "no-preference" }],
    });

    for (const scenario of SCENARIOS) {
      for (const theme of THEMES) {
        const url = `${BASE}/tests/loading-animation/regression.html?scenario=${scenario}&theme=${theme}`;
        await cdp.send("Page.navigate", { url });
        await waitForScene(cdp, scenario);
        // 等 PagerLayout 的旧层淡出，避免截到 out 层。
        await sleep(450);
        const measured = await cdp.send("Runtime.evaluate", {
          expression: MEASURE_EXPRESSION,
          returnByValue: true,
        });
        const value = measured.result?.value;
        if (!value?.ok) {
          throw new Error(
            `测量失败：${scenario}/${theme} ${JSON.stringify({
              value,
              exception: measured.exceptionDetails?.exception?.description,
              text: measured.exceptionDetails?.text,
            })}`
          );
        }
        measurements.push(value);
        const shot = await cdp.send("Page.captureScreenshot", {
          format: "png",
          fromSurface: true,
          captureBeyondViewport: false,
        });
        const file = path.join(OUT_DIR, `${PREFIX}${scenario}-${theme}.png`);
        await writeFile(file, Buffer.from(shot.data, "base64"));
        process.stdout.write(
          `${scenario}/${theme} inkVsArtSlot=${value.offsets.inkVsArtSlotCenter}px ` +
            `inkVsCardContent=${value.offsets.inkVsCardContentCenter}px ` +
            `loaderVsAvailable=${value.offsets.loaderVsAvailableCenter}px ` +
            `inkClearanceTop=${value.clipping.clearanceTop}px ` +
            `inkClearanceBottom=${value.clipping.clearanceBottom}px ` +
            `viewBox="${value.svg.viewBox}" overflow=${value.overflow.horizontalOverflow}\n`
        );
      }
    }

    // 减弱动效：循环挖矿动画必须停用，卡片入场动画也必须关闭。
    const reducedScenario = SCENARIOS[0] ?? "vanilla";
    await cdp.send("Emulation.setEmulatedMedia", {
      media: "screen",
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });
    await cdp.send("Page.navigate", {
      url: `${BASE}/tests/loading-animation/regression.html?scenario=${reducedScenario}&theme=dark`,
    });
    await waitForScene(cdp, reducedScenario);
    await sleep(300);
    const reducedProbe = await cdp.send("Runtime.evaluate", {
      expression: `(() => {
        const loader = document.querySelector(".ni-loading");
        if (!loader) return { ok: false };
        const pick = loader.querySelector(".ni-loading__pick");
        const oreX = loader.querySelector(".ni-loading__ore-x");
        const running = loader
          .getAnimations({ subtree: true })
          .filter((animation) => animation.playState === "running").length;
        return {
          ok: true,
          running,
          pickAnimation: pick ? getComputedStyle(pick).animationName : null,
          loaderAnimation: getComputedStyle(loader).animationName,
          error: loader.classList.contains("ni-loading--error"),
          errorX: oreX ? getComputedStyle(oreX).opacity : null,
        };
      })()`,
      returnByValue: true,
    });
    reducedMotion = reducedProbe.result?.value ?? null;
    const reducedShot = await cdp.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    });
    await writeFile(
      path.join(OUT_DIR, `${PREFIX}${reducedScenario}-reduced-dark.png`),
      Buffer.from(reducedShot.data, "base64")
    );
    process.stdout.write(
      `${reducedScenario}/reduce running=${reducedMotion?.running} pickAnimation=${reducedMotion?.pickAnimation} loaderAnimation=${reducedMotion?.loaderAnimation} errorX=${reducedMotion?.errorX}\n`
    );

  } catch (error) {
    runError = error;
  } finally {
    try {
      socket?.close();
    } catch {
      /* ignore */
    }
    edge.kill();
    await sleep(600);
    await rm(profile, { recursive: true, force: true }).catch(() => {});
  }

  const output = path.join(OUT_DIR, `measurements-${LABEL}.json`);
  const verdict = evaluateAssertions(measurements);
  if (pageErrors.length > 0) {
    verdict.failures.push(`页面运行期异常：${pageErrors.join(" | ")}`);
  }
  if (reducedMotion?.ok) {
    if (reducedMotion.running !== 0 || reducedMotion.pickAnimation !== "none") {
      verdict.failures.push(
        `减弱动效下挖矿动画仍在运行：running=${reducedMotion.running} animationName=${reducedMotion.pickAnimation}`
      );
    } else {
      verdict.notes.push(
        `减弱动效：加载卡片子树 0 个 running 动画，镐子 animation-name=none，卡片入场 animation-name=${reducedMotion.loaderAnimation}`
      );
    }
    if (reducedMotion.error && Number(reducedMotion.errorX) < 0.99) {
      verdict.failures.push(
        `减弱动效下错误叉号未显示：opacity=${reducedMotion.errorX}`
      );
    }
  } else {
    verdict.failures.push("减弱动效探针未取到加载卡片");
  }
  await writeFile(
    output,
    JSON.stringify(
      {
        base: BASE,
        viewport: { width: 900, height: 600 },
        generatedAt: new Date().toISOString(),
        measurements,
        pageErrors,
        reducedMotion,
        verdict,
      },
      null,
      2
    )
  );
  process.stdout.write(`\n测量结果：${output}\n`);
  for (const note of verdict.notes) process.stdout.write(`NOTE ${note}\n`);
  for (const failure of verdict.failures) process.stdout.write(`FAIL ${failure}\n`);
  if (runError) {
    console.error(runError);
    process.exitCode = 1;
    return;
  }
  if (STRICT && verdict.failures.length > 0) {
    process.exitCode = 1;
    return;
  }
  process.stdout.write(`PASS ${measurements.length} 个场景/明暗组合\n`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
