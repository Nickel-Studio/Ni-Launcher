import assert from "node:assert/strict";
import test, { after } from "node:test";
import { createServer } from "vite";

const virtualModules = new Map([
  [
    "@tauri-apps/api/core",
    `
      export async function invoke(command, args) {
        return globalThis.__catalogInvoke(command, args);
      }
      export function convertFileSrc(path) { return path; }
    `,
  ],
  [
    "@tauri-apps/api/event",
    `
      export function listen() { return Promise.resolve(() => {}); }
    `,
  ],
  [
    "@tauri-apps/plugin-opener",
    `
      export async function openPath() {}
      export async function revealItemInDir() {}
      export async function openUrl() {}
    `,
  ],
]);

globalThis.window = {
  __TAURI_INTERNALS__: {
    invoke(command, args) {
      return globalThis.__catalogInvoke(command, args);
    },
    convertFileSrc(path) {
      return path;
    },
  },
};
const server = await createServer({
  configFile: false,
  root: process.cwd(),
  appType: "custom",
  logLevel: "silent",
  server: { middlewareMode: true, hmr: false },
  plugins: [
    {
      name: "catalog-test-mocks",
      enforce: "pre",
      resolveId(source) {
        if (virtualModules.has(source)) return `\0catalog-test:${source}`;
        return null;
      },
      load(id) {
        const source = id.startsWith("\0catalog-test:")
          ? id.slice("\0catalog-test:".length)
          : "";
        return virtualModules.get(source) ?? null;
      },
    },
  ],
});

after(async () => {
  await server.close();
  delete globalThis.window;
  delete globalThis.__catalogInvoke;
});

const {
  cachedCatalog,
  initialCatalog,
  preloadLatestCatalogs,
  requestCatalog,
  schedulePreloadCatalog,
} = await server.ssrLoadModule(
  "/src/components/vanilla/catalogCache.ts"
);
const { normalizeLoaders } = await server.ssrLoadModule(
  "/src/components/download/parse.ts"
);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(probe, label, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = probe();
    if (value) return value;
    await sleep(10);
  }
  throw new Error(`等待超时：${label}`);
}

function version(id) {
  return { id, type: "release", releaseTime: "2026-09-19T14:07:47Z" };
}

function build(loader, tag) {
  return {
    version: `${tag}-${loader}`,
    label: `${tag}-${loader}`,
    releaseTime: "2026-09-19T14:07:47Z",
    modifiedTime: "",
    stable: true,
    latest: true,
    recommended: false,
    downloadUrl: "",
    changelogUrl: "",
    capability: { install: true, saveInstaller: true },
  };
}

function slice(loader, tag, delayMs = 0) {
  return {
    loader,
    builds: [build(loader, tag)],
    provider: {
      state: "available",
      capability: { install: true, saveInstaller: true },
      count: 1,
      error: "",
    },
    cached: false,
    elapsedMs: delayMs,
  };
}

test("fast providers publish before a slow OptiFine request finishes", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    const elapsed = args.loader === "optifine" ? 140 : 5;
    await sleep(elapsed);
    return JSON.stringify(slice(args.loader, "fast", elapsed));
  };

  const current = version("1.20.4-fast");
  let publishFabric;
  const fabricArrived = new Promise((resolve) => {
    publishFabric = resolve;
  });
  const first = requestCatalog(
    current,
    "official_auto",
    false,
    "progressive",
    (progress) => {
      if (progress.changed === "fabric") publishFabric(progress);
    }
  );
  const merged = requestCatalog(current, "official_auto", false, "progressive");
  assert.equal(first, merged);

  const early = await Promise.race([
    fabricArrived,
    sleep(100).then(() => {
      throw new Error("Fabric 未在慢 OptiFine 完成前发布");
    }),
  ]);
  assert.equal(early.catalog.fabric.length, 1);
  assert.equal(early.catalog.providers.optifine.state, "loading");
  assert.equal(early.done, false);

  const complete = await first;
  assert.equal(complete.providers.optifine.state, "available");
  assert.equal(calls.length, 4);
  assert.equal(calls.some((call) => call.loader === "liteloader"), false);

  const hot = await requestCatalog(current, "official_auto", false, "hot");
  assert.equal(calls.length, 4);
  assert.equal(hot.providers.optifine.state, "available");
  assert.equal(cachedCatalog(current, "official_auto").forge[0].version, "fast-forge");
  assert.equal(
    cachedCatalog({ ...current, releaseTime: "2026-09-20T00:00:00Z" }, "official_auto"),
    null
  );
  assert.equal(cachedCatalog(current, "official_first"), null);
});

test("manual force refresh replaces cache and stale responses are ignored", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    const fresh = args.forceRefresh === true;
    const elapsed = fresh ? 5 : 120;
    await sleep(elapsed);
    return JSON.stringify(slice(args.loader, fresh ? "fresh" : "stale", elapsed));
  };

  const current = version("1.20.4-force");
  const stale = requestCatalog(current, "mirror_first", false, "old");
  await sleep(15);
  const fresh = await requestCatalog(current, "mirror_first", true, "new");
  assert.equal(fresh.forge[0].version, "fresh-forge");
  assert.equal(calls.filter((call) => call.forceRefresh).length, 4);

  await stale;
  await sleep(0);
  const cached = cachedCatalog(current, "mirror_first");
  assert.equal(cached.forge[0].version, "fresh-forge");
});

test("a failed provider does not block successful providers", async () => {
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    await sleep(5);
    if (args.loader === "fabric") throw new Error("HTTP 502");
    return JSON.stringify(slice(args.loader, "partial"));
  };

  const current = version("1.20.4-failure");
  const catalog = await requestCatalog(current, "official_first", false, "failure");
  assert.equal(catalog.providers.fabric.state, "failed");
  assert.equal(catalog.providers.fabric.error, "HTTP 502");
  assert.equal(catalog.providers.forge.state, "available");
  assert.equal(catalog.providers.neoforge.state, "available");
  assert.equal(catalog.forge.length, 1);
});

test("OptiFine artifact metadata keeps forge requirement forms without URLs", () => {
  for (const requirement of [null, "", "11.15.1.2318", "2318"]) {
    const catalog = normalizeLoaders({
      optifine: [
        {
          version: "1.8.0_HD_U_I7",
          label: "OptiFine 1.8.0 HD U I7",
          downloadUrl: "https://example.invalid/optifine.jar",
          capability: { install: true, saveInstaller: true },
          artifact: {
            url: "https://example.invalid/secret.jar",
            downloadCandidates: ["https://example.invalid/candidate.jar"],
            profile: { mainClass: "secret.Profile" },
            rawMetadata: {
              minecraftId: "1.8.0",
              patch: "I7",
              forgeRequirement: requirement,
              installerUrl: "https://example.invalid/installer.jar",
              source: "official",
              token: "secret-token",
              nested: { unsafe: true },
            },
          },
        },
      ],
    });

    const artifact = catalog.optifine[0].artifact;
    assert.ok(artifact);
    assert.equal(artifact.rawMetadata.forgeRequirement, requirement);
    assert.equal(artifact.rawMetadata.minecraftId, "1.8.0");
    assert.equal("url" in artifact, false);
    assert.equal("downloadCandidates" in artifact, false);
    assert.equal("profile" in artifact, false);
    assert.equal(artifact.rawMetadata.installerUrl, undefined);
    assert.equal(artifact.rawMetadata.source, undefined);
    assert.equal(artifact.rawMetadata.token, undefined);
    assert.equal(artifact.rawMetadata.nested, undefined);
  }

});

test("local coarse screening avoids inapplicable provider requests", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    await sleep(5);
    return JSON.stringify(slice(args.loader, "coarse"));
  };

  const current = {
    ...version("1.12.2"),
    releaseTime: "2017-09-18T00:00:00Z",
  };
  const catalog = await requestCatalog(
    current,
    "official_auto",
    false,
    "coarse"
  );
  assert.deepEqual(
    [...new Set(calls.map((call) => call.loader))].sort(),
    ["forge", "liteloader", "optifine"]
  );
  assert.equal(catalog.providers.neoforge.state, "hidden");
  assert.equal(catalog.providers.fabric.state, "hidden");
  assert.equal(catalog.liteloader.length, 1);
  assert.equal(
    initialCatalog(version("1.5.2")).providers.liteloader.state,
    "hidden"
  );
  assert.equal(
    initialCatalog(version("1.6.4")).providers.liteloader.state,
    "hidden"
  );
  assert.equal(
    initialCatalog(version("1.7.10")).providers.liteloader.state,
    "loading"
  );
});

test("an empty provider stays unavailable instead of failed", async () => {
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    await sleep(5);
    return JSON.stringify({
      loader: args.loader,
      builds: [],
      provider: {
        state: "unavailable",
        capability: { install: false, saveInstaller: false },
        count: 0,
        error: "",
      },
      cached: false,
      elapsedMs: 5,
    });
  };

  const current = version("1.20.4-empty");
  const catalog = await requestCatalog(
    current,
    "official_auto",
    false,
    "empty"
  );
  assert.equal(catalog.providers.forge.state, "unavailable");
  assert.equal(catalog.providers.optifine.state, "unavailable");
  assert.notEqual(catalog.providers.forge.state, "failed");
  assert.ok(cachedCatalog(current, "official_auto"));
});

test("all visible provider failures still reject with hidden rows excluded", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    throw new Error("HTTP 503");
  };

  const current = {
    ...version("1.12.2"),
    releaseTime: "2017-09-19T00:00:00Z",
  };
  await assert.rejects(
    requestCatalog(current, "official_auto", false, "all-failed"),
    /HTTP 503/
  );
  assert.deepEqual(
    [...new Set(calls.map((call) => call.loader))].sort(),
    ["forge", "liteloader", "optifine"]
  );
});

test("page preload warms only the latest release and snapshot once", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    await sleep(5);
    return JSON.stringify(slice(args.loader, "preload"));
  };

  const release = version("1.20.4-prewarm-release");
  const snapshot = {
    ...version("1.20.4-prewarm-snapshot"),
    type: "snapshot",
  };
  const manifest = {
    latestRelease: release.id,
    latestSnapshot: snapshot.id,
    versions: [snapshot, release],
  };
  preloadLatestCatalogs(manifest, "official_auto");
  preloadLatestCatalogs(manifest, "official_auto");

  await waitFor(
    () =>
      cachedCatalog(release, "official_auto") &&
      cachedCatalog(snapshot, "official_auto"),
    "最新正式版与预览版预热完成"
  );
  assert.equal(calls.length, 8);
  assert.deepEqual(
    [...new Set(calls.map((call) => call.mcVersion))].sort(),
    [release.id, snapshot.id].sort()
  );
});

test("a user catalog request preempts queued preloads", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    await sleep(40);
    return JSON.stringify(slice(args.loader, "preempt"));
  };

  const release = version("1.20.4-preempt-release");
  const snapshot = {
    ...version("1.20.4-preempt-snapshot"),
    type: "snapshot",
  };
  const user = version("1.20.4-preempt-user");
  preloadLatestCatalogs(
    {
      latestRelease: release.id,
      latestSnapshot: snapshot.id,
      versions: [snapshot, release],
    },
    "official_auto"
  );

  await requestCatalog(user, "official_auto", false, "user-selection");
  await sleep(120);

  const requestedVersions = new Set(calls.map((call) => call.mcVersion));
  assert.equal(requestedVersions.has(user.id), true);
  assert.equal(requestedVersions.has(release.id), true);
  assert.equal(requestedVersions.has(snapshot.id), false);
  assert.equal(calls.length, 8);
});

test("rapid hover preloads are debounced to the last candidate", async () => {
  const calls = [];
  globalThis.__catalogInvoke = async (command, args) => {
    assert.equal(command, "list_loader_provider");
    calls.push(args);
    await sleep(5);
    return JSON.stringify(slice(args.loader, "hover"));
  };

  const first = version("1.20.4-hover-first");
  const last = version("1.20.4-hover-last");
  schedulePreloadCatalog(first, "official_auto");
  await sleep(60);
  schedulePreloadCatalog(last, "official_auto");
  await sleep(80);
  assert.equal(calls.length, 0);

  await waitFor(() => calls.length === 4, "最后一次 hover 预热");
  assert.equal(calls.every((call) => call.mcVersion === last.id), true);
});
