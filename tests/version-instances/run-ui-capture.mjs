/**
 * 实例页 / 实例设置 900×600 明暗视觉与布局回归。
 *
 * 前置：pnpm dev -- --port 1504
 * 运行：node tests/version-instances/run-ui-capture.mjs
 *
 * 使用系统 Edge 的 DevTools Protocol，不下载任何浏览器。
 * 证据写入 design/instance-ui 与 design/instance-settings（两者均被 .gitignore 忽略，
 * 属于本地验收产物，不入库）。
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

const PORT = 9334;
const BASE =
  process.env.INSTANCES_TEST_BASE ?? "http://127.0.0.1:1504";
const UI_ARTIFACTS = path.resolve("design/instance-ui");
const SETTINGS_ARTIFACTS = path.resolve("design/instance-settings");
const PROFILE = path.join(UI_ARTIFACTS, `edge-profile-${process.pid}`);
const VIEW_WIDTH = 900;
const VIEW_HEIGHT = 600;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await mkdir(UI_ARTIFACTS, { recursive: true });
await mkdir(SETTINGS_ARTIFACTS, { recursive: true });
await rm(PROFILE, { recursive: true, force: true });

const checks = [];
const screenshots = [];

function check(name, ok, detail = "") {
  checks.push({ name, ok, detail });
  if (!ok) throw new Error(`${name}${detail ? `：${detail}` : ""}`);
}

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

async function waitFor(cdp, expression, label, timeout = 12000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await evaluate(cdp, expression).catch(() => false);
    if (value) return value;
    await sleep(100);
  }
  throw new Error(`等待超时：${label}`);
}

async function navigate(cdp, query, readyExpression) {
  await cdp.send("Page.navigate", {
    url: `${BASE}/tests/version-instances/regression.html${query}`,
  });
  await waitFor(
    cdp,
    "document.readyState === 'complete' && Boolean(document.querySelector('#root'))",
    "页面加载"
  );
  await waitFor(
    cdp,
    "document.documentElement.dataset.regression === 'visual'",
    "视觉模式"
  );
  if (readyExpression) {
    await waitFor(cdp, readyExpression, "目标内容");
  }
}

async function setTheme(cdp, theme) {
  await evaluate(
    cdp,
    `(() => {
      document.documentElement.dataset.theme = '${theme}';
      document.documentElement.dataset.accent = 'mono';
    })()`
  );
  // 颜色主题令牌切换有 420ms 插值，截图前等待过渡结束。
  await sleep(620);
}

async function screenshot(cdp, directory, filename) {
  const result = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  const target = path.join(directory, filename);
  await writeFile(target, Buffer.from(result.data, "base64"));
  screenshots.push(target);
  return target;
}

async function scrollTo(cdp, expression, label) {
  const ok = await evaluate(
    cdp,
    `(() => {
      const target = ${expression};
      if (!target) return false;
      // 页面滚动容器可能是 window / body / 分页容器：逐个祖先设置 scrollTop。
      let node = target.parentElement;
      let scrolled = false;
      while (node) {
        if (node.scrollHeight > node.clientHeight + 4) {
          node.scrollTop = Math.max(0, target.getBoundingClientRect().top + node.scrollTop - node.getBoundingClientRect().top - 12);
          scrolled = true;
        }
        node = node.parentElement;
      }
      if (!scrolled) {
        window.scrollTo(0, target.getBoundingClientRect().top + window.scrollY - 12);
      }
      return true;
    })()`
  );
  if (!ok) throw new Error(`找不到滚动目标：${label}`);
  await sleep(220);
}

const groupExpr = (title) =>
  `Array.from(document.querySelectorAll('.vst-group')).find((el) => el.getAttribute('aria-label') === ${JSON.stringify(
    title
  )})`;

const rowExpr = (label) =>
  `Array.from(document.querySelectorAll('.vst-row')).find((row) => row.querySelector('.vst-row__label')?.textContent?.trim() === ${JSON.stringify(
    label
  )})`;

async function hoverElement(cdp, expression) {
  const rect = await evaluate(
    cdp,
    `(() => {
      const el = ${expression};
      if (!el) return null;
      el.scrollIntoView({ block: 'center' });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    })()`
  );
  if (!rect) throw new Error("悬浮目标不存在");
  await sleep(120);
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseMoved",
    x: rect.x,
    y: rect.y,
  });
  await sleep(520);
}

async function moveAway(cdp) {
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseMoved",
    x: 4,
    y: 4,
  });
  await sleep(120);
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
    `--window-size=${VIEW_WIDTH},${VIEW_HEIGHT}`,
    "about:blank",
  ],
  { stdio: "ignore" }
);

let cdp;
let failure = null;
const consoleErrors = [];

try {
  const targets = await waitForJson(`http://127.0.0.1:${PORT}/json/list`);
  const page = targets.find((target) => target.type === "page");
  if (!page?.webSocketDebuggerUrl) {
    throw new Error("系统 Edge 未提供可连接的页面目标");
  }
  cdp = await Cdp.connect(page.webSocketDebuggerUrl);
  cdp.on("Runtime.exceptionThrown", (params) => {
    consoleErrors.push(
      params.exceptionDetails?.exception?.description ?? "JS 异常"
    );
  });
  cdp.on("Runtime.consoleAPICalled", (params) => {
    if (params.type === "error") {
      consoleErrors.push(
        params.args.map((item) => item.value ?? item.description ?? "").join(" ")
      );
    }
  });
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: VIEW_WIDTH,
    height: VIEW_HEIGHT,
    deviceScaleFactor: 1,
    mobile: false,
  });

  // ---------------- 实例页：标题与回退后的卡片布局 ----------------
  await navigate(cdp, "?visual=instances", "document.querySelectorAll('.vs-card').length >= 3");
  await setTheme(cdp, "dark");
  const instancesLayout = await evaluate(
    cdp,
    `(() => {
      const title = document.querySelector('.vs-head__title')?.textContent?.trim() ?? '';
      const root = document.querySelector('.vs-root');
      const grid = document.querySelector('.vs-grid');
      return {
        title,
        display: root ? getComputedStyle(root).display : '',
        columns: root ? getComputedStyle(root).gridTemplateColumns : '',
        cards: document.querySelectorAll('.vs-card').length,
        gridDisplay: grid ? getComputedStyle(grid).display : '',
        overflow: document.documentElement.scrollWidth > window.innerWidth,
      };
    })()`
  );
  check("实例页标题为「实例与版本」", instancesLayout.title === "实例与版本", instancesLayout.title);
  check(
    "实例页保持目录导航 + 实例卡片布局",
    instancesLayout.display === "grid" &&
      instancesLayout.columns.includes("268px") &&
      instancesLayout.cards >= 3 &&
      instancesLayout.gridDisplay === "grid",
    JSON.stringify(instancesLayout)
  );
  check("实例页 900×600 无横向溢出", instancesLayout.overflow === false);
  await screenshot(cdp, UI_ARTIFACTS, "instances-dark.png");
  await setTheme(cdp, "light");
  await screenshot(cdp, UI_ARTIFACTS, "instances-light.png");

  // ---------------- 导航文案：More 中的入口名称 ----------------
  await navigate(cdp, "?visual=more", "document.querySelectorAll('.mo-card__action').length === 4");
  const moreText = await evaluate(cdp, "document.body.textContent ?? ''");
  check("指南入口使用「实例与版本」", moreText.includes("实例与版本"));
  check("指南入口不再出现「版本选择」", !moreText.includes("版本选择"));

  // ---------------- 实例设置：字段顺序与行布局 ----------------
  await navigate(cdp, "?visual=settings", "document.querySelectorAll('.vst-group').length >= 4");
  await setTheme(cdp, "dark");
  const settingsStructure = await evaluate(
    cdp,
    `(() => {
      const groups = Array.from(document.querySelectorAll('.vst-group'));
      const groupLabels = groups.map((el) => el.getAttribute('aria-label') ?? '');
      const labelsOf = (title) => {
        const group = groups.find((el) => el.getAttribute('aria-label') === title);
        return group
          ? Array.from(group.querySelectorAll('.vst-row__label')).map((el) => el.textContent?.trim() ?? '')
          : [];
      };
      const rows = Array.from(document.querySelectorAll('.vst-row'));
      const rowLayout = rows.map((row) => {
        const control = row.querySelector('.vst-row__control');
        const rr = row.getBoundingClientRect();
        const cr = control?.getBoundingClientRect();
        return {
          display: getComputedStyle(row).display,
          rightAligned: cr ? Math.abs(cr.right - rr.right) <= 1.5 : false,
          overflow: row.scrollWidth > row.clientWidth + 1,
        };
      });
      const label = ${rowExpr("游戏窗口标题")}?.querySelector('.vst-row__label');
      const windowRow = ${rowExpr("游戏窗口标题")};
      const windowInput = windowRow?.querySelector('input');
      const windowToggle = windowRow?.querySelector('.vst-inherit__toggle');
      const customInfoRow = ${rowExpr("自定义信息")};
      const authRow = ${rowExpr("认证服务器")};
      return {
        groupLabels,
        startup: labelsOf('启动选项'),
        memory: labelsOf('内存分配'),
        maxSliderPresent: Boolean(
          document.querySelector('.vst-row input[type="range"][aria-label="最大内存"]')
        ),
        server: labelsOf('服务器'),
        advanced: labelsOf('高级选项'),
        rowLayout,
        inheritModeNodes: document.querySelectorAll('.vst-inherit__mode').length,
        inheritDirection: (() => {
          const el = document.querySelector('.vst-inherit');
          return el ? getComputedStyle(el).flexDirection : '';
        })(),
        inheritedInputDisabled: windowInput ? windowInput.disabled : null,
        inheritedToggleText: windowToggle?.textContent?.trim() ?? '',
        inheritedToggleStyle: windowToggle
          ? {
              background: getComputedStyle(windowToggle).backgroundColor,
              color: getComputedStyle(windowToggle).color,
            }
          : null,
        customInfoClear: Boolean(customInfoRow?.querySelector('.vst-inherit__clear')),
        authClear: Boolean(authRow?.querySelector('.vst-inherit__clear')),
        stateBadges: document.querySelectorAll('.vst-inherit__state').length,
        labelCursor: label ? getComputedStyle(label).cursor : '',
        labelDecoration: label ? getComputedStyle(label).textDecorationLine : '',
        helpOnControl: Boolean(windowRow?.querySelector('.vst-row__control-inner')),
        text: document.body.textContent ?? '',
        overflow: document.documentElement.scrollWidth > window.innerWidth,
      };
    })()`
  );
  check(
    "版块顺序为启动选项 → 内存分配 → 服务器 → 高级选项",
    JSON.stringify(settingsStructure.groupLabels.slice(0, 4)) ===
      JSON.stringify(["启动选项", "内存分配", "服务器", "高级选项"]),
    JSON.stringify(settingsStructure.groupLabels)
  );
  check(
    "启动选项字段顺序为版本隔离 → 游戏窗口标题 → 自定义信息 → Java",
    JSON.stringify(settingsStructure.startup) ===
      JSON.stringify(["版本隔离", "游戏窗口标题", "自定义信息", "Java"]),
    JSON.stringify(settingsStructure.startup)
  );
  check(
    "内存分配字段顺序为分配方式 → 最大内存",
    JSON.stringify(settingsStructure.memory) ===
      JSON.stringify(["分配方式", "最大内存"]),
    JSON.stringify(settingsStructure.memory)
  );
  check(
    "自定义模式显示最大内存 Slider",
    settingsStructure.maxSliderPresent === true
  );
  // 分配方式切换：自动模式不显示自定义滑块，切回自定义后恢复。
  const switchMode = async (label) => {
    const clicked = await evaluate(
      cdp,
      `(() => {
        const tab = Array.from(document.querySelectorAll('.nw-seg [role="tab"]')).find(
          (el) => el.textContent?.trim() === ${JSON.stringify(label)}
        );
        if (!tab) return false;
        tab.click();
        return true;
      })()`
    );
    if (!clicked) throw new Error(`找不到分配方式：${label}`);
  };
  await switchMode("自动");
  await waitFor(
    cdp,
    "!Array.from(document.querySelectorAll('.vst-row__label')).some((el) => el.textContent?.trim() === '最大内存')",
    "自动模式隐藏最大内存"
  );
  check("自动分配时不显示最大内存 Slider", true);
  await switchMode("自定义");
  await waitFor(
    cdp,
    "Boolean(document.querySelector('.vst-row input[type=\"range\"][aria-label=\"最大内存\"]'))",
    "自定义模式恢复最大内存"
  );
  check("切回自定义恢复最大内存 Slider", true);
  check(
    "服务器字段含条件显示的三项",
    JSON.stringify(settingsStructure.server) ===
      JSON.stringify(["登录方式", "认证服务器", "注册链接", "服务器名称", "自动进入服务器"]),
    JSON.stringify(settingsStructure.server)
  );
  check(
    "高级选项字段顺序覆盖参数、内存管理、启动前命令与兼容性开关",
    JSON.stringify(settingsStructure.advanced) ===
      JSON.stringify([
        "JVM 参数",
        "游戏参数",
        "内存管理",
        "启动前执行命令",
        "禁止更新 Mod",
        "关闭文件校验",
        "禁用 Java Launch Wrapper",
        "禁用 LWJGL Unsafe Agent",
      ]),
    JSON.stringify(settingsStructure.advanced)
  );
  check(
    "所有设置行使用标签列 + 控件列网格并贴右",
    settingsStructure.rowLayout.every((row) => row.display === "grid" && row.rightAligned),
    JSON.stringify(settingsStructure.rowLayout)
  );
  check(
    "设置行内部无横向溢出",
    settingsStructure.rowLayout.every((row) => !row.overflow)
  );
  check(
    "继承文本不再是来源 + 清空的整行三件套",
    settingsStructure.inheritModeNodes === 0 &&
      settingsStructure.inheritDirection === "row"
  );
  check(
    "继承文本只读展示全局值且切换为「跟随全局」",
    settingsStructure.inheritedInputDisabled === true &&
      settingsStructure.inheritedToggleText === "跟随全局",
    JSON.stringify({
      disabled: settingsStructure.inheritedInputDisabled,
      toggle: settingsStructure.inheritedToggleText,
    })
  );
  check(
    "清空只在单独设置且非空时出现",
    settingsStructure.authClear === true && settingsStructure.customInfoClear === false
  );
  check(
    "继承布尔用紧凑标签表达全局值",
    settingsStructure.stateBadges >= 4,
    String(settingsStructure.stateBadges)
  );
  check(
    "标签为普通文本（无 help 光标 / 下划线）",
    settingsStructure.labelCursor !== "pointer" &&
      settingsStructure.labelDecoration === "none",
    JSON.stringify({
      cursor: settingsStructure.labelCursor,
      decoration: settingsStructure.labelDecoration,
    })
  );
  check("说明 Tooltip 挂在控件容器上", settingsStructure.helpOnControl === true);
  check("实例设置 900×600 无横向溢出", settingsStructure.overflow === false);
  check("不再出现「运行内存 / 刷新内存」冗余行", !settingsStructure.text.includes("运行内存") && !settingsStructure.text.includes("刷新内存"));
  check("不再出现「打开注册页面」", !settingsStructure.text.includes("打开注册页面"));
  check("不再出现「全局默认」只读行", !settingsStructure.text.includes("全局默认"));
  check("内存管理不再提供串行 / 并行 GC", !settingsStructure.text.includes("串行") && !settingsStructure.text.includes("并行"));

  for (const theme of ["dark", "light"]) {
    await setTheme(cdp, theme);
    for (const [title, filename] of [
      ["启动选项", `settings-${theme}-startup.png`],
      ["内存分配", `settings-${theme}-memory.png`],
      ["服务器", `settings-${theme}-server.png`],
      ["高级选项", `settings-${theme}-advanced.png`],
    ]) {
      await scrollTo(cdp, groupExpr(title), title);
      await screenshot(cdp, SETTINGS_ARTIFACTS, filename);
    }
  }

  // ---------------- Tooltip：悬浮控件才出现 ----------------
  await setTheme(cdp, "dark");
  await scrollTo(cdp, rowExpr("游戏窗口标题"), "游戏窗口标题");
  await hoverElement(cdp, `${rowExpr("游戏窗口标题")}?.querySelector('.vst-row__label')`);
  const labelTip = await evaluate(
    cdp,
    "document.querySelector('.nw-tooltip')?.textContent?.trim() ?? ''"
  );
  check("悬浮标签不出现 Tooltip", labelTip === "", labelTip);
  await hoverElement(
    cdp,
    `${rowExpr("游戏窗口标题")}?.querySelector('.vst-row__control-inner')`
  );
  const controlTip = await evaluate(
    cdp,
    "document.querySelector('.nw-tooltip')?.textContent?.trim() ?? ''"
  );
  check(
    "悬浮控件出现 Tooltip",
    controlTip.includes("支持 {date}") && controlTip.includes("游戏运行时自动刷新"),
    controlTip
  );
  await screenshot(cdp, SETTINGS_ARTIFACTS, "settings-dark-tooltip.png");
  await moveAway(cdp);
  await setTheme(cdp, "light");
  await hoverElement(
    cdp,
    `${rowExpr("游戏窗口标题")}?.querySelector('.vst-row__control-inner')`
  );
  await screenshot(cdp, SETTINGS_ARTIFACTS, "settings-light-tooltip.png");
  await moveAway(cdp);

  // ---------------- 概览：快捷方式四列等宽 + 高级管理贴右 ----------------
  await navigate(cdp, "?visual=overview", "document.querySelectorAll('.vst-shortcuts .vst-shortcut').length === 4");
  await setTheme(cdp, "dark");
  const overviewStructure = await evaluate(
    cdp,
    `(() => {
      const shortcuts = document.querySelector('.vst-shortcuts');
      const columns = shortcuts ? getComputedStyle(shortcuts).gridTemplateColumns : '';
      const buttons = Array.from(document.querySelectorAll('.vst-shortcut'));
      const widths = buttons.map((el) => Math.round(el.getBoundingClientRect().width));
      const inset = buttons.map((el) => getComputedStyle(el).boxShadow.includes('inset'));
      const advanced = document.querySelector('[aria-label="高级管理"]');
      const rows = advanced ? Array.from(advanced.querySelectorAll('.vst-row')) : [];
      const rightAligned = rows.map((row) => {
        const control = row.querySelector('.vst-row__control');
        if (!control) return false;
        return Math.abs(control.getBoundingClientRect().right - row.getBoundingClientRect().right) <= 1.5;
      });
      return {
        columns: columns.split(' ').filter(Boolean).length,
        widths,
        inset,
        rightAligned,
        overflow: document.documentElement.scrollWidth > window.innerWidth,
      };
    })()`
  );
  check(
    "快捷方式为四列等宽网格",
    overviewStructure.columns === 4 &&
      overviewStructure.widths.length === 4 &&
      Math.max(...overviewStructure.widths) - Math.min(...overviewStructure.widths) <= 1,
    JSON.stringify(overviewStructure)
  );
  check(
    "快捷方式保持 1px 内描边线框",
    overviewStructure.inset.every(Boolean),
    JSON.stringify(overviewStructure.inset)
  );
  check(
    "高级管理按钮贴右",
    overviewStructure.rightAligned.length > 0 &&
      overviewStructure.rightAligned.every(Boolean),
    JSON.stringify(overviewStructure.rightAligned)
  );
  check("实例概览 900×600 无横向溢出", overviewStructure.overflow === false);

  for (const theme of ["dark", "light"]) {
    await setTheme(cdp, theme);
    await scrollTo(cdp, groupExpr("快捷方式"), "快捷方式");
    await screenshot(cdp, SETTINGS_ARTIFACTS, `overview-${theme}-shortcuts.png`);
    await scrollTo(cdp, groupExpr("高级管理"), "高级管理");
    await screenshot(cdp, SETTINGS_ARTIFACTS, `overview-${theme}-advanced.png`);
  }

  // ---------------- reduced-motion：设置页过渡被关闭 ----------------
  await cdp.send("Emulation.setEmulatedMedia", {
    features: [
      { name: "prefers-color-scheme", value: "dark" },
      { name: "prefers-reduced-motion", value: "reduce" },
    ],
  });
  await navigate(cdp, "?visual=settings", "document.querySelectorAll('.vst-group').length >= 4");
  await setTheme(cdp, "dark");
  const reduced = await evaluate(
    cdp,
    `(() => {
      const input = document.querySelector('.vst-input');
      const toggle = document.querySelector('.vst-inherit__toggle');
      return {
        inputTransition: input ? getComputedStyle(input).transitionDuration : '',
        toggleTransition: toggle ? getComputedStyle(toggle).transitionDuration : '',
      };
    })()`
  );
  const durations = [reduced.inputTransition, reduced.toggleTransition]
    .join(",")
    .split(",")
    .map((value) => value.trim());
  check(
    "reduced-motion 下设置控件过渡关闭",
    durations.length > 0 && durations.every((value) => value === "0s"),
    JSON.stringify(reduced)
  );
  await screenshot(cdp, SETTINGS_ARTIFACTS, "settings-reduced-motion.png");

  check("页面控制台无错误", consoleErrors.length === 0, consoleErrors.join("\n"));

  const output = {
    status: "pass",
    viewport: { width: VIEW_WIDTH, height: VIEW_HEIGHT },
    checks,
    screenshots,
    debug: { inheritedToggle: settingsStructure.inheritedToggleStyle },
  };
  await writeFile(
    path.join(UI_ARTIFACTS, "capture-report.json"),
    `${JSON.stringify(output, null, 2)}\n`
  );
  console.log(
    JSON.stringify(
      {
        status: output.status,
        passed: checks.filter((item) => item.ok).length,
        failed: checks.filter((item) => !item.ok).length,
        screenshots,
      },
      null,
      2
    )
  );
} catch (error) {
  failure = error;
  await writeFile(
    path.join(UI_ARTIFACTS, "capture-report.json"),
    `${JSON.stringify(
      {
        status: "fail",
        error: error instanceof Error ? error.message : String(error),
        checks,
        screenshots,
      },
      null,
      2
    )}\n`
  ).catch(() => undefined);
} finally {
  cdp?.close();
  if (edge.pid) {
    spawnSync("taskkill", ["/PID", String(edge.pid), "/T", "/F"], {
      stdio: "ignore",
    });
  }
  await rm(PROFILE, { recursive: true, force: true }).catch(() => undefined);
}

if (failure) {
  console.error(failure instanceof Error ? failure.stack : String(failure));
  process.exitCode = 1;
}
