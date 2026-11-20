/** node tests/mods-scan/run-regression.mjs；仅复用系统 Edge 与仓库 Vite，不安装依赖。 */
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import path from "node:path";

const artifacts = path.resolve("design/mods-progressive");
const profile = path.join(artifacts, `edge-profile-${process.pid}`);
const edgePath = [process.env.EDGE_PATH, ...[process.env["ProgramFiles(x86)"], process.env.ProgramFiles].filter(Boolean).map((base) => path.join(base, "Microsoft/Edge/Application/msedge.exe"))].find((file) => file && existsSync(file));
if (!edgePath) throw new Error("找不到系统 Edge；请指定 EDGE_PATH 后重试。");
await mkdir(artifacts, { recursive: true });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function availablePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}
async function json(url) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try { const response = await fetch(url); if (response.ok) return await response.json(); } catch { /* 等待本次测试服务启动。 */ }
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
        clearTimeout(item.timer); this.pending.delete(message.id);
        if (message.error) item.reject(new Error(message.error.message)); else item.resolve(message.result);
      } else if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
      else if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") errors.push(message.params.args.map((item) => item.value ?? item.description).join(" "));
    });
  }
  send(method, params = {}) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`CDP 超时：${method}`)); }, 15_000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  }
  async navigate(url, expected) {
    await this.send("Page.navigate", { url });
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline) {
      const state = await this.evaluate("document.documentElement.dataset.regression").catch(() => "");
      if (state === expected) return;
      if (state === "fail") {
        const detail = await this.evaluate("({summary: document.getElementById('regression-result').textContent, active:document.activeElement?.outerHTML, focusVisible:document.activeElement?.matches(':focus-visible'), documentFocus:document.hasFocus(), tips:document.querySelectorAll('[role=tooltip]').length})");
        await this.screenshot("regression-failure.png");
        throw new Error(JSON.stringify(detail, null, 2));
      }
      await sleep(60);
    }
    throw new Error(`页面状态等待超时：${url}`);
  }
  async screenshot(name) {
    const result = await this.send("Page.captureScreenshot", { format: "png", fromSurface: true, captureBeyondViewport: false });
    await writeFile(path.join(artifacts, name), Buffer.from(result.data, "base64"));
    return name;
  }
}

const devPort = await availablePort();
const cdpPort = await availablePort();
const base = process.env.MODS_TEST_BASE ?? `http://127.0.0.1:${devPort}`;
const vite = process.env.MODS_TEST_BASE ? null : spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", String(devPort)], { stdio: "ignore" });
const edge = spawn(edgePath, ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-extensions", `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, "--window-size=900,600", "about:blank"], { stdio: "ignore" });
const errors = [];
let socket;
const report = { checks: [], screenshots: [], visual: [], errors };
try {
  const targets = await json(`http://127.0.0.1:${cdpPort}/json/list`);
  socket = new WebSocket(targets.find((target) => target.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.addEventListener("open", resolve, { once: true }); socket.addEventListener("error", reject, { once: true }); });
  const cdp = new Cdp(socket, errors);
  await cdp.send("Page.enable"); await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 900, height: 600, deviceScaleFactor: 1, mobile: false });
  for (const motion of ["no-preference", "reduce"]) {
    console.log(`运行 900×600 ${motion} 回归`);
    await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: motion }] });
    await cdp.navigate(`${base}/tests/mods-scan/regression.html?motion=${motion}`, "pass");
    report.checks.push({ motion, checks: await cdp.evaluate("window.__modsRegression.checks") });
  }
  for (const [kind, mode] of [["mods", "dark"], ["placeholder", "dark"], ["icons", "dark"], ["failure", "dark"], ["game", "dark"], ["settings", "light"], ["loading", "dark"]]) {
    await cdp.navigate(`${base}/tests/mods-scan/regression.html?visual=${kind}`, "visual");
    await cdp.evaluate(`document.documentElement.dataset.theme = '${mode}'`);
    await sleep(500);
    if (kind === "game" || kind === "settings") {
      const titleSelector =
        kind === "game"
          ? 'input[aria-label="游戏窗口标题"]'
          : 'button[aria-label^="游戏窗口标题："]';
      const expectedPrefix = "游戏窗口标题";
      const targetState = await cdp.evaluate(`({disabled: document.querySelector('${titleSelector}')?.disabled, hidden: document.querySelector('${titleSelector}')?.offsetParent === null})`);
      console.log("target", kind, targetState);
      await cdp.evaluate(`document.querySelector('${titleSelector}').focus()`);
      // 真键盘焦点：先 Tab 离开，再 Shift+Tab 回到控件，确保 :focus-visible 生效。
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9, modifiers: 8 });
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9, modifiers: 8 });
      await sleep(100);
      const focus = await cdp.evaluate(`({focused: document.activeElement?.getAttribute('aria-label'), activeTag: document.activeElement?.tagName, focusVisible: document.activeElement?.matches(':focus-visible'), tip: document.querySelector('[role=tooltip]')?.textContent, animation: document.querySelector('[role=tooltip]') && getComputedStyle(document.querySelector('[role=tooltip]')).animationName})`);
      if (!focus.focused?.startsWith(expectedPrefix) || !focus.tip?.includes("{date}") || focus.animation !== "none") throw new Error(`键盘或 reduced-motion Tooltip 失败：${JSON.stringify(focus)}`);
      report.visual.push({ kind, focus });
    }
    if (kind === "loading") {
      const running = await cdp.evaluate("document.querySelector('.ni-loading') ? document.querySelector('.ni-loading').getAnimations({subtree:true}).filter(animation => animation.playState === 'running').length : -1");
      if (running !== 0) throw new Error(`reduced-motion 加载动画未停：${running}`);
    }
    const bounds = await cdp.evaluate("({width:innerWidth,height:innerHeight,scrollWidth:document.documentElement.scrollWidth,bodyWidth:document.body.scrollWidth})");
    if (bounds.scrollWidth > bounds.width || bounds.bodyWidth > bounds.width) throw new Error(`小窗口横向溢出：${JSON.stringify(bounds)}`);
    report.visual.push({ kind, mode, bounds });
    report.screenshots.push(await cdp.screenshot(`${kind}-${mode}-900x600.png`));
  }
  if (errors.length) throw new Error(`控制台异常：${errors.join("\n")}`);
  report.status = "pass";
  console.log(JSON.stringify({ status: report.status, checks: report.checks.map(({ motion, checks }) => ({ motion, passed: checks.filter((check) => check.ok).length })), screenshots: report.screenshots }, null, 2));
} catch (error) {
  report.status = "fail"; report.failure = String(error); process.exitCode = 1; console.error(error);
} finally {
  await writeFile(path.join(artifacts, "regression-result.json"), JSON.stringify(report, null, 2));
  socket?.close();
  for (const child of [edge, vite]) if (child?.pid && child.pid !== process.pid) spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  await rm(profile, { recursive: true, force: true }).catch(() => undefined);
}
