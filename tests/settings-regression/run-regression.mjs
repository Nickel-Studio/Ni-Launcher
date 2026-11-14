/** node tests/settings-regression/run-regression.mjs；仅复用系统 Edge 与仓库 Vite，不安装依赖。 */
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import path from "node:path";

const artifacts = path.resolve("design/settings-memory");
const profile = path.join(artifacts, `edge-profile-${process.pid}`);
const edgePath = [
  process.env.EDGE_PATH,
  ...[process.env["ProgramFiles(x86)"], process.env.ProgramFiles]
    .filter(Boolean)
    .map((base) => path.join(base, "Microsoft/Edge/Application/msedge.exe")),
].find((file) => file && existsSync(file));
if (!edgePath) throw new Error("找不到系统 Edge；请指定 EDGE_PATH 后重试。");
await mkdir(artifacts, { recursive: true });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function availablePort() {
  const server = createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function json(url) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return await response.json();
    } catch {
      // 等待本次测试服务启动。
    }
    await sleep(100);
  }
  throw new Error(`启动超时：${url}`);
}

class Cdp {
  next = 0;
  pending = new Map();
  constructor(socket, errors) {
    this.socket = socket;
    socket.addEventListener("message", ({ data }) => {
      const message = JSON.parse(String(data));
      if (message.id) {
        const item = this.pending.get(message.id);
        if (!item) return;
        clearTimeout(item.timer);
        this.pending.delete(message.id);
        if (message.error) item.reject(new Error(message.error.message));
        else item.resolve(message.result);
      } else if (message.method === "Runtime.exceptionThrown") {
        errors.push(
          message.params.exceptionDetails.exception?.description ??
            message.params.exceptionDetails.text
        );
      } else if (
        message.method === "Runtime.consoleAPICalled" &&
        message.params.type === "error"
      ) {
        errors.push(
          message.params.args
            .map((item) => item.value ?? item.description)
            .join(" ")
        );
      }
    });
  }
  send(method, params = {}) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP 超时：${method}`));
      }, 15_000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (result.exceptionDetails) {
      throw new Error(
        result.exceptionDetails.exception?.description ??
          result.exceptionDetails.text
      );
    }
    return result.result.value;
  }
  async screenshot(name) {
    const result = await this.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    });
    await writeFile(path.join(artifacts, name), Buffer.from(result.data, "base64"));
    return name;
  }
  async moveMouse(x, y) {
    await this.send("Input.dispatchMouseEvent", {
      type: "mouseMoved",
      x,
      y,
      button: "none",
      buttons: 0,
    });
  }
  async tab() {
    await this.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Tab",
      code: "Tab",
      windowsVirtualKeyCode: 9,
    });
    await this.send("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Tab",
      code: "Tab",
      windowsVirtualKeyCode: 9,
    });
  }
}

async function waitFor(cdp, expression, timeout = 20_000, label = expression) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await cdp.evaluate(expression);
      if (last) return last;
    } catch {
      // 页面尚未就绪。
    }
    await sleep(80);
  }
  throw new Error(`等待超时：${label}；最后结果：${JSON.stringify(last)}`);
}

async function waitForRegression(cdp, url) {
  await cdp.send("Page.navigate", { url });
  const deadline = Date.now() + 40_000;
  while (Date.now() < deadline) {
    const state = await cdp
      .evaluate("document.documentElement.dataset.regression || 'running'")
      .catch(() => "running");
    if (state === "pass") return "pass";
    if (state === "fail") {
      const summary = await cdp
        .evaluate(
          "document.getElementById('regression-result')?.textContent ?? ''"
        )
        .catch(() => "");
      throw new Error(`既有 UI 回归失败：${url}\n${summary}`);
    }
    await sleep(100);
  }
  throw new Error(`既有 UI 回归超时：${url}`);
}

const devPort = await availablePort();
const cdpPort = await availablePort();
const base = process.env.SETTINGS_TEST_BASE ?? `http://127.0.0.1:${devPort}`;
const vite = process.env.SETTINGS_TEST_BASE
  ? null
  : spawn(
      process.execPath,
      [
        "node_modules/vite/bin/vite.js",
        "--host",
        "127.0.0.1",
        "--port",
        String(devPort),
      ],
      { stdio: "ignore" }
    );
const edge = spawn(
  edgePath,
  [
    "--headless=new",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    `--remote-debugging-port=${cdpPort}`,
    `--user-data-dir=${profile}`,
    "--window-size=900,600",
    "about:blank",
  ],
  { stdio: "ignore" }
);

const errors = [];
let socket;
const report = { checks: [], screenshots: [], uiRegressions: [], errors };

function check(name, ok, detail) {
  report.checks.push({ name, ok: Boolean(ok), detail });
  if (!ok) throw new Error(`检查失败：${name}\n${JSON.stringify(detail, null, 2)}`);
}

try {
  const targets = await json(`http://127.0.0.1:${cdpPort}/json/list`);
  socket = new WebSocket(
    targets.find((target) => target.type === "page").webSocketDebuggerUrl
  );
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  const cdp = new Cdp(socket, errors);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 900,
    height: 600,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await cdp.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-reduced-motion", value: "no-preference" }],
  });

  await cdp.send("Page.navigate", {
    url: `${base}/tests/settings-regression/regression.html`,
  });
  await waitFor(
    cdp,
    "!!window.__settingsRegression && document.querySelectorAll('.st-row').length > 3",
    20_000,
    "GamePanel 挂载"
  );
  await waitFor(
    cdp,
    "document.querySelector('.st-game-memory__bar') && document.querySelector('.st-game-memory__used')?.getBoundingClientRect().width > 0",
    20_000,
    "内存条数据"
  );

  const label = await cdp.evaluate(`(() => {
    const row = [...document.querySelectorAll('.st-row')].find(
      (item) => item.querySelector('.st-row__label')?.textContent === '游戏窗口标题'
    );
    const label = row?.querySelector('.st-row__label');
    if (!label) return null;
    const style = getComputedStyle(label);
    return {
      cursor: style.cursor,
      decoration: style.textDecorationLine,
      tabIndex: label.getAttribute('tabindex'),
      helpSpans: document.querySelectorAll('.st-setting-help').length,
      controlTag: row.querySelector('.st-row__control')?.tagName ?? '',
    };
  })()`);
  check(
    "标签保持普通文本",
    label &&
      label.cursor !== "help" &&
      label.decoration === "none" &&
      label.tabIndex === null &&
      label.helpSpans === 0 &&
      label.controlTag === "DIV",
    label
  );

  const rect = await cdp.evaluate(`(() => {
    const row = [...document.querySelectorAll('.st-row')].find(
      (item) => item.querySelector('.st-row__label')?.textContent === '游戏窗口标题'
    );
    const control = row?.querySelector('.st-row__control');
    const text = row?.querySelector('.st-row__label');
    control?.scrollIntoView({ block: 'center' });
    const c = control.getBoundingClientRect();
    const l = text.getBoundingClientRect();
    return {
      control: { x: c.x + c.width / 2, y: c.y + c.height / 2 },
      label: { x: l.x + l.width / 2, y: l.y + l.height / 2 },
    };
  })()`);
  await sleep(100);
  const topTip = await cdp.evaluate(
    "document.querySelector('[role=tooltip]')?.textContent ?? ''"
  );
  check("初始不显示 Tooltip", topTip === "", { topTip });
  await cdp.moveMouse(rect.control.x, rect.control.y);
  await sleep(520);
  const hoverTip = await cdp.evaluate(
    "document.querySelector('[role=tooltip]')?.textContent ?? ''"
  );
  check("悬浮控件显示 Tooltip", hoverTip.includes("{date}"), { hoverTip });
  await cdp.moveMouse(rect.label.x, rect.label.y);
  await sleep(450);
  const labelTip = await cdp.evaluate(
    "document.querySelector('[role=tooltip]')?.textContent ?? ''"
  );
  check("悬浮标签不显示 Tooltip", labelTip === "", { labelTip });

  await cdp.evaluate(
    "document.querySelector('input[aria-label=\"游戏窗口标题\"]')?.focus()"
  );
  await sleep(120);
  let focus = await cdp.evaluate(`(() => ({
    label: document.activeElement?.getAttribute('aria-label'),
    focusVisible: document.activeElement?.matches(':focus-visible') ?? false,
    tip: document.querySelector('[role=tooltip]')?.textContent ?? '',
  }))()`);
  if (!focus.tip) {
    await cdp.evaluate(
      "document.querySelector('[aria-label=\"默认版本隔离\"]')?.focus()"
    );
    await cdp.tab();
    await sleep(120);
    focus = await cdp.evaluate(`(() => ({
      label: document.activeElement?.getAttribute('aria-label'),
      focusVisible: document.activeElement?.matches(':focus-visible') ?? false,
      tip: document.querySelector('[role=tooltip]')?.textContent ?? '',
    }))()`);
  }
  check(
    "键盘聚焦控件显示 Tooltip",
    focus.label === "游戏窗口标题" &&
      focus.focusVisible === true &&
      focus.tip.includes("{date}"),
    focus
  );
  await cdp.evaluate(
    "document.querySelector('input[aria-label=\"游戏窗口标题\"]')?.blur()"
  );

  const bar = await cdp.evaluate(`(() => {
    const bar = document.querySelector('.st-game-memory__bar');
    const used = document.querySelector('.st-game-memory__used');
    const allocated = document.querySelector('.st-game-memory__allocated');
    const empty = document.querySelector('.st-game-memory__empty');
    const width = (el) => el.getBoundingClientRect().width;
    const style = getComputedStyle(allocated);
    return {
      total: width(bar),
      used: width(used),
      allocated: width(allocated),
      empty: width(empty),
      transitionDuration: style.transitionDuration,
      transitionTimingFunction: style.transitionTimingFunction,
      values: [...document.querySelectorAll('.st-game-memory__values dd')].map(
        (item) => item.textContent
      ),
    };
  })()`);
  check(
    "内存条三段宽度相加 100%",
    Math.abs(bar.used + bar.allocated + bar.empty - bar.total) < 1.5,
    bar
  );
  check(
    "内存条 800ms ease-out",
    bar.transitionDuration.split(",").every((duration) => duration.trim() === "0.8s") &&
      bar.transitionTimingFunction.includes("cubic-bezier(0.16, 1, 0.3, 1)"),
    bar
  );
  check(
    "内存文本只保留已使用/游戏分配",
    bar.values.length === 2 &&
      bar.values[0].includes("6.2 GB") &&
      bar.values[0].includes("16.0 GB") &&
      bar.values[1].includes("8.0 GB"),
    bar.values
  );

  const beforePoll = await cdp.evaluate("window.__settingsRegression.pollCount");
  await cdp.evaluate("window.__settingsRegression.setAvailable(9000)");
  await sleep(1300);
  const afterPoll = await cdp.evaluate("window.__settingsRegression.pollCount");
  const polledUsed = await cdp.evaluate(
    "document.querySelector('.st-game-memory__values dd')?.textContent ?? ''"
  );
  check(
    "运行时轮询实时更新",
    afterPoll > beforePoll && polledUsed.includes("7.2 GB"),
    { beforePoll, afterPoll, polledUsed }
  );

  const allocatedBefore = await cdp.evaluate(
    "document.querySelector('.st-game-memory__allocated').getBoundingClientRect().width"
  );
  await cdp.evaluate(`(() => {
    const input = document.querySelector('input[aria-label="游戏内存"]');
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      'value'
    ).set;
    setter.call(input, '4096');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await sleep(60);
  const animations = await cdp.evaluate(`(() =>
    document.querySelector('.st-game-memory__allocated').getAnimations().map((item) => ({
      duration: item.effect.getTiming().duration,
      state: item.playState,
    }))
  )()`);
  await sleep(900);
  const allocatedAfter = await cdp.evaluate(
    "document.querySelector('.st-game-memory__allocated').getBoundingClientRect().width"
  );
  check(
    "切换分配值有 800ms 动画",
    allocatedAfter < allocatedBefore - 1 &&
      animations.some((item) => item.duration === 800),
    { allocatedBefore, allocatedAfter, animations }
  );

  const hiddenSupported = await cdp.evaluate(`(() => {
    try {
      window.__settingsRegressionHidden = false;
      Object.defineProperty(document, 'hidden', {
        configurable: true,
        get: () => window.__settingsRegressionHidden,
      });
      return true;
    } catch {
      return false;
    }
  })()`);
  if (hiddenSupported) {
    await cdp.evaluate(
      "window.__settingsRegressionHidden = true; document.dispatchEvent(new Event('visibilitychange'))"
    );
    const hiddenBefore = await cdp.evaluate("window.__settingsRegression.pollCount");
    await sleep(1500);
    const hiddenAfter = await cdp.evaluate("window.__settingsRegression.pollCount");
    check("页面隐藏时暂停轮询", hiddenAfter === hiddenBefore, {
      hiddenBefore,
      hiddenAfter,
    });
    await cdp.evaluate(
      "window.__settingsRegressionHidden = false; document.dispatchEvent(new Event('visibilitychange'))"
    );
    await sleep(1300);
    const visibleAfter = await cdp.evaluate("window.__settingsRegression.pollCount");
    check("恢复可见后继续轮询", visibleAfter > hiddenAfter, {
      hiddenAfter,
      visibleAfter,
    });
  } else {
    report.checks.push({
      name: "页面隐藏时暂停轮询",
      ok: true,
      detail: "当前浏览器不允许覆写 document.hidden，跳过该检查",
    });
  }

  const usageBeforeFail = await cdp.evaluate(
    "document.querySelector('.st-game-memory__values dd')?.textContent ?? ''"
  );
  await cdp.evaluate("window.__settingsRegression.setFail(true)");
  await sleep(1300);
  const failure = await cdp.evaluate(`(() => ({
    error: document.querySelector('.st-game-memory__error')?.textContent ?? '',
    usage: document.querySelector('.st-game-memory__values dd')?.textContent ?? '',
  }))()`);
  check(
    "轮询失败保留上次值并显示行内错误",
    failure.error.includes("模拟读取失败") && failure.usage === usageBeforeFail,
    { usageBeforeFail, failure }
  );
  await cdp.evaluate("window.__settingsRegression.setFail(false)");
  await sleep(1300);
  const recovered = await cdp.evaluate(
    "document.querySelector('.st-game-memory__error')?.textContent ?? ''"
  );
  check("轮询恢复后清除错误", recovered === "", { recovered });

  await cdp.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-reduced-motion", value: "reduce" }],
  });
  await sleep(100);
  const reduced = await cdp.evaluate(`(() => {
    const allocated = document.querySelector('.st-game-memory__allocated');
    const style = getComputedStyle(allocated);
    const input = document.querySelector('input[aria-label="游戏内存"]');
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      'value'
    ).set;
    setter.call(input, '6144');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
    return {
      duration: style.transitionDuration,
      animations: allocated.getAnimations().length,
    };
  })()`);
  check(
    "reduced-motion 关闭宽度过渡",
    reduced.duration.split(",").every((duration) => duration.trim() === "0s") &&
      reduced.animations === 0,
    reduced
  );
  await cdp.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-reduced-motion", value: "no-preference" }],
  });

  await cdp.evaluate("document.getElementById('regression-result').style.display = 'none'");
  await cdp.evaluate("window.scrollTo(0, 0)");
  await cdp.evaluate("document.documentElement.dataset.theme = 'dark'");
  await sleep(300);
  report.screenshots.push(await cdp.screenshot("settings-top-dark-900x600.png"));
  await cdp.evaluate("document.querySelector('#game-memory')?.scrollIntoView({ block: 'start' })");
  await sleep(300);
  report.screenshots.push(await cdp.screenshot("settings-memory-dark-900x600.png"));
  await cdp.evaluate("document.documentElement.dataset.theme = 'light'");
  await sleep(300);
  report.screenshots.push(await cdp.screenshot("settings-memory-light-900x600.png"));
  await cdp.evaluate("window.scrollTo(0, 0)");
  await sleep(300);
  report.screenshots.push(await cdp.screenshot("settings-top-light-900x600.png"));

  if (errors.length) {
    throw new Error(`控制台异常：${errors.join("\n")}`);
  }

  // 浏览器预览（无 Tauri IPC）：GameMemory 不得持续产生轮询错误。
  const errorsBeforePreview = errors.length;
  report.uiRegressions.push({
    route: "settings-preview",
    status: await waitForRegression(
      cdp,
      `${base}/tests/settings-regression/preview.html`
    ),
  });
  if (errors.length > errorsBeforePreview) {
    report.checks.push({
      name: "浏览器预览控制台",
      ok: false,
      detail: errors.slice(errorsBeforePreview),
    });
  }

  const errorsBeforeExisting = errors.length;
  report.uiRegressions.push({
    route: "ui-remaining",
    status: await waitForRegression(
      cdp,
      `${base}/tests/ui-remaining/regression.html`
    ),
  });
  report.uiRegressions.push({
    route: "ui-foundation",
    status: await waitForRegression(
      cdp,
      `${base}/tests/ui-foundation/regression.html`
    ),
  });
  if (errors.length > errorsBeforeExisting) {
    report.checks.push({
      name: "既有 UI 回归控制台",
      ok: false,
      detail: errors.slice(errorsBeforeExisting),
    });
  }

  report.status = report.checks.every((item) => item.ok) ? "pass" : "fail";
  console.log(
    JSON.stringify(
      {
        status: report.status,
        checks: report.checks.map((item) => ({
          name: item.name,
          ok: item.ok,
        })),
        screenshots: report.screenshots,
        uiRegressions: report.uiRegressions,
      },
      null,
      2
    )
  );
} catch (error) {
  report.status = "fail";
  report.failure = String(error);
  process.exitCode = 1;
  console.error(error);
} finally {
  await writeFile(
    path.join(artifacts, "regression-result.json"),
    JSON.stringify(report, null, 2)
  );
  socket?.close();
  for (const child of [edge, vite]) {
    if (child?.pid && child.pid !== process.pid) {
      spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
        stdio: "ignore",
      });
    }
  }
  await rm(profile, { recursive: true, force: true }).catch(() => undefined);
}
