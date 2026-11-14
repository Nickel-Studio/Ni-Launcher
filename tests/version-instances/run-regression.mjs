/**
 * 无额外依赖的系统 Edge 回归/截图脚本。
 *
 * 前置：pnpm dev -- --port 1504
 * 运行：node tests/version-instances/run-regression.mjs
 *
 * 不下载 Playwright 浏览器；通过系统 Edge 的 DevTools Protocol 打开真实页面，
 * 运行 fixture 内断言，并补充明暗模式、reduced-motion 与截图。
 */
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";

const EDGE_CANDIDATES = [
  process.env.EDGE_PATH,
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
].filter(Boolean);
const EDGE = EDGE_CANDIDATES.find((candidate) => existsSync(candidate));
if (!EDGE) {
  throw new Error("未找到系统 Microsoft Edge；本脚本不会下载浏览器。");
}

const PORT = 9333;
// 默认沿用历史端口；已有 Vite 时可指向它，避免重复启动开发服务器。
const BASE = process.env.INSTANCES_TEST_BASE ?? "http://127.0.0.1:1504";
const ARTIFACTS = path.resolve("design/ui-instances");
const PROFILE = path.join(ARTIFACTS, `edge-profile-${process.pid}`);
await mkdir(ARTIFACTS, { recursive: true });
await rm(PROFILE, { recursive: true, force: true });

const consoleErrors = [];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

class Cdp {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 0;
    this.pending = new Map();
    this.listeners = new Map();
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(message.error.message));
        else pending.resolve(message.result);
        return;
      }
      if (message.method) {
        for (const listener of this.listeners.get(message.method) ?? []) {
          listener(message.params);
        }
      }
    });
  }

  static async connect(url) {
    const socket = new WebSocket(url);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    return new Cdp(socket);
  }

  on(method, listener) {
    const listeners = this.listeners.get(method) ?? [];
    listeners.push(listener);
    this.listeners.set(method, listeners);
  }

  send(method, params = {}) {
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket.close();
  }
}

async function waitForJson(url, timeout = 12000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return await response.json();
    } catch {
      // Edge 启动中，继续等待。
    }
    await sleep(100);
  }
  throw new Error(`等待 DevTools 端点超时：${url}`);
}

async function evaluate(cdp, expression) {
  const result = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(
      result.exceptionDetails.exception?.description ??
        result.exceptionDetails.text ??
        "页面脚本执行失败"
    );
  }
  return result.result.value;
}

async function navigate(cdp, url) {
  await cdp.send("Page.navigate", { url });
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) {
    const ready = await evaluate(
      cdp,
      "document.readyState === 'complete' && Boolean(document.querySelector('#root'))"
    ).catch(() => false);
    if (ready) return;
    await sleep(100);
  }
  throw new Error(`页面加载超时：${url}`);
}

async function waitForState(cdp, expected, timeout = 15000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const state = await evaluate(
      cdp,
      "document.documentElement.dataset.regression || ''"
    ).catch(() => "");
    if (state === expected) return;
    if (state === "fail") return;
    await sleep(100);
  }
  throw new Error(`等待页面状态超时：${expected}`);
}

async function waitForInstances(cdp, count = 3) {
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) {
    const ready = await evaluate(
      cdp,
      `document.querySelectorAll('.vs-card').length >= ${count}`
    ).catch(() => false);
    if (ready) return;
    await sleep(100);
  }
  throw new Error(`等待实例卡片超时：${count}`);
}

async function screenshot(cdp, filename) {
  const result = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  const target = path.join(ARTIFACTS, filename);
  await writeFile(target, Buffer.from(result.data, "base64"));
  return target;
}

const edge = spawn(
  EDGE,
  [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    `--remote-debugging-port=${PORT}`,
    "--remote-allow-origins=*",
    `--user-data-dir=${PROFILE}`,
    "--window-size=1180,900",
    "about:blank",
  ],
  { stdio: "ignore" }
);

let cdp;
try {
  const targets = await waitForJson(`http://127.0.0.1:${PORT}/json/list`);
  const page = targets.find((target) => target.type === "page");
  if (!page?.webSocketDebuggerUrl) {
    throw new Error("系统 Edge 未提供可连接的页面目标");
  }
  cdp = await Cdp.connect(page.webSocketDebuggerUrl);
  cdp.on("Runtime.exceptionThrown", (params) => {
    consoleErrors.push(params.exceptionDetails?.exception?.description ?? "JS 异常");
  });
  cdp.on("Runtime.consoleAPICalled", (params) => {
    if (params.type === "error") {
      consoleErrors.push(
        params.args
          .map((item) => item.value ?? item.description ?? "")
          .join(" ")
      );
    }
  });
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 1180,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  });

  await navigate(cdp, `${BASE}/tests/version-instances/regression.html`);
  await waitForState(cdp, "pass");
  const report = await evaluate(
    cdp,
    `(() => ({
      status: document.documentElement.dataset.regression,
      checks: window.__versionInstancesRegression?.checks ?? [],
      summary: document.querySelector('#regression-result')?.textContent ?? ''
    }))()`
  );
  if (report.status !== "pass") {
    throw new Error(
      `浏览器回归失败：\n${report.checks
        .filter((check) => !check.ok)
        .map((check) => `${check.name}${check.detail ? ` · ${check.detail}` : ""}`)
        .join("\n")}`
    );
  }

  const themeStyles = async (theme) => {
    // dataset.regression 在 render() 后同步写入，React 提交可能稍晚；先等卡片落地。
    await evaluate(
      cdp,
      `(() => {
        document.documentElement.dataset.theme = '${theme}';
        document.documentElement.dataset.accent = 'mono';
      })()`
    );
    // 颜色主题令牌是注册色值，切换后有 420ms 插值；截图和断言必须等过渡结束。
    await sleep(620);
    return evaluate(
      cdp,
      `(() => {
        const card = document.querySelector('[data-instance-id="26.3-fabric"]');
        const tool = card?.querySelector('.vs-card__favorite');
        return {
          accent: document.documentElement.dataset.accent ?? '',
          card: card ? getComputedStyle(card).backgroundColor : '',
          tool: tool ? getComputedStyle(tool).backgroundColor : '',
          toolVisible: Boolean(tool),
        };
      })()`
    );
  };

  await navigate(cdp, `${BASE}/tests/version-instances/regression.html?visual=instances`);
  await waitForState(cdp, "visual");
  await evaluate(
    cdp,
    "document.querySelector('#regression-result').style.display = 'none'"
  );
  const dark = await themeStyles("dark");
  if (!dark.toolVisible || !dark.card || !dark.tool || dark.accent !== "mono") {
    throw new Error("明暗模式检查缺少实例卡或卡片工具按钮");
  }
  await screenshot(cdp, "instances-dark-menu.png");

  const light = await themeStyles("light");
  if (light.card === dark.card || light.tool === dark.tool) {
    throw new Error(`明暗模式背景未切换：${JSON.stringify({ dark, light })}`);
  }
  await screenshot(cdp, "instances-light-menu.png");

  await cdp.send("Emulation.setEmulatedMedia", {
    features: [
      { name: "prefers-color-scheme", value: "dark" },
      { name: "prefers-reduced-motion", value: "reduce" },
    ],
  });
  await navigate(cdp, `${BASE}/tests/version-instances/regression.html?visual=instances`);
  await waitForState(cdp, "visual");
  await evaluate(
    cdp,
    `(() => {
      document.querySelector('#regression-result').style.display = 'none';
      document.documentElement.dataset.theme = 'dark';
      document.documentElement.dataset.accent = 'mono';
    })()`
  );
  await sleep(620);
  const reduced = await evaluate(
    cdp,
    `(() => {
      const card = document.querySelector('[data-instance-id="26.3-fabric"]');
      const tool = card?.querySelector('.vs-card__favorite');
      return {
        cardAnimation: card ? getComputedStyle(card).animationName : '',
        toolTransition: tool ? getComputedStyle(tool).transitionDuration : '',
      };
    })()`
  );
  if (
    reduced.cardAnimation !== "none" ||
    !String(reduced.toolTransition)
      .split(",")
      .every((value) => value.trim() === "0s")
  ) {
    throw new Error(`reduced-motion 未关闭动效：${JSON.stringify(reduced)}`);
  }
  await screenshot(cdp, "instances-reduced-motion.png");

  await cdp.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-color-scheme", value: "dark" }],
  });
  await navigate(cdp, `${BASE}/tests/version-instances/regression.html?visual=instances`);
  await waitForState(cdp, "visual");
  await evaluate(
    cdp,
    `(() => {
      document.querySelector('#regression-result').style.display = 'none';
      document.documentElement.dataset.theme = 'dark';
      document.documentElement.dataset.accent = 'mono';
    })()`
  );
  await sleep(620);
  const instancesShot = await screenshot(cdp, "instances-dark.png");

  await navigate(cdp, `${BASE}/tests/version-instances/regression.html?visual=more`);
  await waitForState(cdp, "visual");
  await evaluate(
    cdp,
    `(() => {
      document.querySelector('#regression-result').style.display = 'none';
      document.documentElement.dataset.theme = 'dark';
      document.documentElement.dataset.accent = 'mono';
    })()`
  );
  await sleep(620);
  await screenshot(cdp, "more-dark.png");

  if (consoleErrors.length) {
    throw new Error(`页面控制台错误：\n${consoleErrors.join("\n")}`);
  }

  const output = {
    status: report.status,
    checks: report.checks,
    theme: { dark, light },
    reducedMotion: reduced,
    screenshots: [
      path.join(ARTIFACTS, "instances-dark-menu.png"),
      path.join(ARTIFACTS, "instances-light-menu.png"),
      path.join(ARTIFACTS, "instances-reduced-motion.png"),
      instancesShot,
      path.join(ARTIFACTS, "more-dark.png"),
    ],
  };
  await writeFile(
    path.join(ARTIFACTS, "regression-result.json"),
    `${JSON.stringify(output, null, 2)}\n`
  );
  console.log(
    JSON.stringify(
      {
        status: output.status,
        passed: output.checks.filter((check) => check.ok).length,
        failed: output.checks.filter((check) => !check.ok).length,
        theme: output.theme,
        reducedMotion: output.reducedMotion,
        screenshots: output.screenshots,
      },
      null,
      2
    )
  );
} finally {
  cdp?.close();
  if (edge.pid) {
    spawnSync("taskkill", ["/PID", String(edge.pid), "/T", "/F"], {
      stdio: "ignore",
    });
  }
  await rm(PROFILE, { recursive: true, force: true }).catch(() => undefined);
}
