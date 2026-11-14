import assert from "node:assert/strict";
import test, { after, beforeEach } from "node:test";
import { createServer } from "vite";

// 使用项目现有 Vite 转译器运行真实 IPC 模块，不新增测试框架或网络请求。
const storage = new Map();
globalThis.window = {
  __TAURI_INTERNALS__: {
    invoke: (command, args) => globalThis.__communityInvoke(command, args),
    convertFileSrc: (path) => path,
  },
  localStorage: {
    getItem: (key) => storage.get(key) ?? null,
    setItem: (key, value) => storage.set(key, value),
    removeItem: (key) => storage.delete(key),
  },
};
const server = await createServer({
  configFile: false,
  root: process.cwd(),
  appType: "custom",
  logLevel: "silent",
  server: { middlewareMode: true, hmr: false },
});
after(async () => {
  await server.close();
  delete globalThis.window;
  delete globalThis.__communityInvoke;
});
const resourceIpc = await server.ssrLoadModule("/src/components/resources/ipc.ts");
const { setLaunchInstance } = await server.ssrLoadModule("/src/lib/instances.ts");
const { loadInstanceFilter } = await server.ssrLoadModule("/src/components/download/ipc.ts");
const { searchModrinth, loadModrinthFilter } = await server.ssrLoadModule("/src/components/modrinth/ipc.ts");
const { searchCurseforge } = await server.ssrLoadModule("/src/components/curseforge/ipc.ts");
const target = { root: "C:/community-fixture", versionId: "arbitrary-directory" };
const calls = [];
let context;
beforeEach(() => {
  calls.length = 0;
  context = {
    resourceDir: target.root,
    displayName: "Foo 26.3",
    minecraftVersion: "1.26.3",
    loader: "fabric",
  };
  setLaunchInstance({ ...target, displayName: "Foo 26.3", loader: "fabric", icon: "fabric" });
  globalThis.__communityInvoke = async (command, args) => {
    calls.push({ command, args });
    if (command === "resource_instance_context") return context;
    if (command === "modrinth_search" || command === "curseforge_search") {
      return JSON.stringify({ total: 0, hits: [] });
    }
    throw new Error(`Unexpected command: ${command}`);
  };
});

for (const minecraftVersion of ["1.26.3", "26.3", "26.3-snapshot-1", "3D Shareware v1.34"]) {
  test(`all filter entry points pass the actual Minecraft version ${minecraftVersion}`, async () => {
    context.minecraftVersion = minecraftVersion;
    for (const load of [resourceIpc.loadResourceInstance, resourceIpc.loadInstanceSearchFilter, loadInstanceFilter, loadModrinthFilter]) {
      const filter = await load();
      assert.equal(filter.mcVersion, minecraftVersion);
      for (const search of [searchModrinth, searchCurseforge]) {
        await search({ kind: "mod", query: "", gameVersion: filter.mcVersion, loader: filter.loader, offset: 0, limit: 10 });
      }
    }
    const requests = calls.filter(({ command }) => command.endsWith("_search"));
    assert.equal(requests.length, 8);
    assert.ok(requests.every(({ args }) => args.gameVersion === minecraftVersion));
    assert.ok(requests.every(({ args }) => args.gameVersion !== context.displayName && args.gameVersion !== target.versionId));
    assert.deepEqual(calls[0].args, { dir: target.root, versionId: target.versionId });
  });
}

test("null, missing, blank and malformed versions stay unknown instead of borrowing names", async () => {
  for (const unknown of [null, undefined, "", "  ", 263]) {
    context.minecraftVersion = unknown;
    const resolved = await resourceIpc.loadResourceInstance();
    assert.equal(resolved.label, "Foo 26.3");
    assert.equal(resolved.mcVersion, "");
    await searchModrinth({ kind: "mod", query: "", gameVersion: resolved.mcVersion, loader: resolved.loader, offset: 0, limit: 10 });
    assert.equal(calls.at(-1).args.gameVersion, "");
    await assert.rejects(resourceIpc.revalidateResourceInstance(resolved), /无法识别实例的 Minecraft 版本/);
  }
});

test("revalidation ignores display-only renames but rejects same-directory version or loader changes", async () => {
  const resolved = await resourceIpc.loadResourceInstance();
  context.displayName = "My renamed pack";
  assert.equal((await resourceIpc.revalidateResourceInstance(resolved)).mcVersion, "1.26.3");
  context.minecraftVersion = "26.3";
  await assert.rejects(resourceIpc.revalidateResourceInstance(resolved), /游戏版本已变化/);
  context.minecraftVersion = "1.26.3";
  context.loader = "forge";
  await assert.rejects(resourceIpc.revalidateResourceInstance(resolved), /游戏版本已变化/);
});

test("revalidation detects a different instance even if both share the same resource directory", async () => {
  const resolved = await resourceIpc.loadResourceInstance();
  setLaunchInstance({ ...target, versionId: "other-directory", loader: "fabric", icon: "fabric" });
  await assert.rejects(resourceIpc.revalidateResourceInstance(resolved), /当前实例/);
  assert.equal((await resourceIpc.revalidateResourceInstance(resolved, target)).versionId, target.versionId);
});

test("a selection change during the context request cannot install into the stale target", async () => {
  const resolved = await resourceIpc.loadResourceInstance();
  let finish;
  globalThis.__communityInvoke = () => new Promise((resolve) => { finish = resolve; });
  const pending = resourceIpc.revalidateResourceInstance(resolved);
  setLaunchInstance({ ...target, versionId: "changed-during-request", loader: "fabric", icon: "fabric" });
  finish(context);
  await assert.rejects(pending, /当前实例/);
});

test("missing explicit targets never fall back to the launch instance and IPC failures propagate", async () => {
  context = null;
  assert.equal(await resourceIpc.loadResourceInstance({ ...target, versionId: "missing" }), null);
  assert.deepEqual(calls[0].args, { dir: target.root, versionId: "missing" });
  assert.equal(calls.length, 1);
  globalThis.__communityInvoke = async () => { throw new Error("读取实例失败"); };
  await assert.rejects(resourceIpc.loadInstanceSearchFilter(target), /读取实例失败/);
});
