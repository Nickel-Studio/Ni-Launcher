// 透明无边框窗口的四个圆角只由 CSS 画出（.ni-shell 的 border-radius + overflow）。
// 挂在 body 下、position: fixed 且铺满整窗的浮层不在 .ni-shell 里，不受它裁剪；
// 它们必须自己声明 border-radius: var(--shell-radius)，否则遮罩底色、backdrop-filter
// 或投影会把四个透明角涂成方块，盖在圆角窗口上。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));

async function cssFiles(dir) {
  const files = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await cssFiles(path)));
    else if (entry.name.endsWith(".css")) files.push(path);
  }
  return files;
}

async function stylesheets() {
  const sheets = [];
  for (const path of await cssFiles(join(root, "src"))) {
    sheets.push({
      name: relative(root, path).split(sep).join("/"),
      source: await readFile(path, "utf8"),
    });
  }
  // 启动闪屏的样式内联在 index.html，必须在 JS 加载前就能画出来。
  const html = await readFile(join(root, "index.html"), "utf8");
  for (const [, source] of html.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)) {
    sheets.push({ name: "index.html", source });
  }
  return sheets;
}

const declares = (body, pattern) =>
  body.split(";").some((declaration) => pattern.test(declaration.trim()));

// 只匹配不含花括号的最内层规则块，@media 里的规则也会被单独取到。
function fullWindowFixedRules({ name, source }) {
  const rules = [];
  const plain = source.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const [, selector, body] of plain.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (
      declares(body, /^position\s*:\s*fixed$/) &&
      declares(body, /^inset\s*:\s*0(px)?$/)
    ) {
      rules.push({ name, selector: selector.trim(), body });
    }
  }
  return rules;
}

test("铺满整窗的 fixed 浮层都带窗口圆角", async () => {
  const rules = (await stylesheets()).flatMap(fullWindowFixedRules);
  // 先确认扫描本身有效：已知的三个整窗浮层都要被找到，避免正则失效后测试空转通过。
  const selectors = rules.map((rule) => rule.selector);
  for (const known of ["#ni-splash", ".ni-modal__backdrop", ".dl-overlay"]) {
    assert.ok(selectors.includes(known), `没有扫描到 ${known}`);
  }
  const missing = rules
    .filter((rule) => !declares(rule.body, /^border-radius\s*:\s*var\(--shell-radius\)$/))
    .map((rule) => `${rule.name}: ${rule.selector}`);
  assert.deepEqual(missing, [], "这些整窗浮层缺少 border-radius: var(--shell-radius)");
});
