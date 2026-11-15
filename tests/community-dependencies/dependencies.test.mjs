import assert from "node:assert/strict";
import test, { after, before, beforeEach } from "node:test";
import { createServer } from "vite";

let server;
let parse;
let deps;
let filters;
let modrinth;
let curseforge;
const calls = [];
const storage = new Map();
let invokeImpl;

const project = (id, slug = id, source = "modrinth") => ({
  projectId: id,
  slug,
  source,
  title: "Same display name",
  projectType: "mod",
  description: "",
  iconUrl: "",
  author: "",
  downloads: 0,
  dateModified: "",
  versions: [],
  loaders: [],
});
const version = (id, projectId = "Parent", gameVersions = ["1.20.1"]) => ({
  id,
  projectId,
  name: id,
  versionNumber: id,
  gameVersions,
  loaders: ["fabric"],
  datePublished: "2026-01-01T00:00:00Z",
  releaseType: "release",
  files: [],
  dependencies: [],
});
const dependency = (projectId = "", versionId = "", fileName = "") => ({
  dependencyType: "required",
  projectId,
  versionId,
  fileName,
});

before(async () => {
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke: (command, args) => {
        calls.push({ command, args });
        return invokeImpl(command, args);
      },
      convertFileSrc: (path) => path,
    },
    localStorage: {
      getItem: (key) => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: (key) => storage.delete(key),
    },
  };
  server = await createServer({
    configFile: false,
    root: process.cwd(),
    appType: "custom",
    logLevel: "silent",
    server: { middlewareMode: true, hmr: false },
  });
  parse = await server.ssrLoadModule("/src/components/modrinth/parse.ts");
  deps = await server.ssrLoadModule("/src/components/community/dependencies.ts");
  filters = await server.ssrLoadModule("/src/components/community/versionFilters.ts");
  modrinth = await server.ssrLoadModule("/src/components/modrinth/ipc.ts");
  curseforge = await server.ssrLoadModule("/src/components/curseforge/ipc.ts");
});
after(async () => {
  await server?.close();
  delete globalThis.window;
});
beforeEach(() => {
  calls.length = 0;
  invokeImpl = async (_command, args) => JSON.stringify({
    project: project("Other", "other"),
    version: args.versionId ? version(args.versionId, "Other") : null,
  });
});

test("Modrinth accepts only required relations and retains API, project-only, version-only and filename-only declarations", () => {
  const normalized = parse.normalizeDependencies([
    { dependency_type: "required", project_id: " fabric-api " },
    { dependency_type: "required", project_id: "quilted-fabric-api" },
    { dependency_type: "required", version_id: "PinnedVersion" },
    { dependency_type: "required", file_name: "external.jar" },
    ...["optional", "incompatible", "embedded", "Required"].map((dependency_type) => ({ dependency_type, project_id: dependency_type })),
    { dependency_type: "required", project_id: "" },
    null,
  ]);
  assert.deepEqual(normalized, [
    dependency("fabric-api"),
    dependency("quilted-fabric-api"),
    dependency("", "PinnedVersion"),
    dependency("", "", "external.jar"),
    dependency(),
  ]);
});

test("CurseForge accepts numeric relationType 3 only, including conflicting metadata", () => {
  assert.deepEqual(parse.normalizeDependencies([
    { relationType: 3, modId: 306612 },
    { relationType: 3, modId: 634179 },
    { relationType: 2, modId: 2 },
    { relationType: 4, modId: 4 },
    { relationType: 5, modId: 5 },
    { relationType: "3", modId: 6 },
    { relationType: null, modId: 7 },
    { relationType: 2, dependencyType: "required", modId: 8 },
    { relationType: 3, dependencyType: "optional", modId: 9 },
  ]), [dependency("306612"), dependency("634179")]);
});

test("normalized camelCase dependencies retain their exact identifiers", () => {
  assert.deepEqual(parse.normalizeDependencies(JSON.stringify([
    dependency("Canonical", "Exact", "pinned.jar"),
  ])), [dependency("Canonical", "Exact", "pinned.jar")]);
});

test("invalid numeric identifiers never become guessed project or version names", () => {
  const values = parse.normalizeDependencies([
    { relationType: 3, modId: -1 },
    { relationType: 3, modId: 0 },
    { relationType: 3, modId: Number.MAX_SAFE_INTEGER + 1 },
    { dependency_type: "required", project_id: {}, version_id: false, file_name: "manual.jar" },
  ]);
  assert.ok(values.every((value) => !value.projectId && !value.versionId));
  assert.equal(values.at(-1).fileName, "manual.jar");
});

test("known-empty and unavailable dependency metadata stay distinct without blocking downloadable main files", () => {
  const file = { url: "https://example.invalid/main.jar", filename: "main.jar", primary: true };
  const records = [undefined, null, {}, []].map((dependencies, index) => ({
    ...version(String(index)), dependencies, files: [file],
  }));
  const normalized = parse.normalizeVersions(records);
  assert.deepEqual(normalized.map((value) => value.dependencies), [undefined, undefined, undefined, []]);
  assert.ok(normalized.every((value) => value.files[0].filename === "main.jar"));
  assert.throws(() => parse.normalizeDependencies({}));
  assert.throws(() => parse.normalizeVersions({}));
});

test("dependencies belong to the concrete parent version, not a project-global union", () => {
  const values = parse.normalizeVersions([
    { ...version("ParentA"), dependencies: [{ dependency_type: "required", version_id: "ExactA" }] },
    { ...version("ParentB"), dependencies: [{ relationType: 3, modId: 634179 }] },
  ]);
  assert.deepEqual(values[0].dependencies, [dependency("", "ExactA")]);
  assert.deepEqual(values[1].dependencies, [dependency("634179")]);
});

test("declaration deduplication removes self references and repeats but preserves different pinned versions and both APIs", () => {
  const result = deps.requiredDependencies([
    dependency("Parent"), dependency("parent-slug"), dependency("", "OwnVersion"),
    dependency("fabric-api"), dependency("fabric-api"), dependency("quilted-fabric-api"),
    dependency("Pinned"), dependency("Pinned", "ExactA"), dependency("Pinned", "ExactA", "same.jar"), dependency("Pinned", "ExactB"),
    dependency("", "", "manual.jar"), dependency("", "", "manual.jar"),
  ], project("Parent", "parent-slug"), version("OwnVersion"));
  assert.deepEqual(result, [
    dependency("fabric-api"), dependency("quilted-fabric-api"),
    dependency("Pinned", "ExactA"), dependency("Pinned", "ExactB"), dependency("", "", "manual.jar"),
  ]);
});

test("filename equality does not collapse different stable projects or different exact versions", () => {
  const result = deps.requiredDependencies([
    dependency("A", "ExactA", "same.jar"), dependency("B", "ExactB", "same.jar"),
    dependency("A", "ExactC", "same.jar"),
  ], project("Parent"), version("Own"));
  assert.equal(result.length, 3);
});

test("resolved aliases and version-only self references deduplicate by platform identity, never display title", () => {
  const parent = project("", "parent-slug");
  const ownVersion = version("OwnVersion", "ActualParent");
  const entry = (declaration, hit, file = null) => ({ dependency: declaration, resolved: { project: hit, version: file } });
  const a = project("OtherA");
  const b = project("OtherB");
  const result = deps.deduplicateResolvedDependencies([
    entry(dependency("", "SelfVersion"), project("ActualParent", "renamed-parent"), version("SelfVersion", "ActualParent")),
    entry(dependency("OtherA"), a),
    entry(dependency("", "ExactA"), a, version("ExactA", "OtherA")),
    entry(dependency("AliasA", "ExactA"), a, version("ExactA", "OtherA")),
    entry(dependency("", "ExactB"), a, version("ExactB", "OtherA")),
    entry(dependency("OtherB"), b),
    entry(dependency("ActualParent"), project("ActualParent", "other-platform", "curseforge")),
  ], parent, ownVersion);
  assert.deepEqual(result.map((item) => [item.resolved.project.source, item.resolved.project.projectId, item.resolved.version?.id]), [
    ["modrinth", "OtherA", "ExactA"], ["modrinth", "OtherA", "ExactB"],
    ["modrinth", "OtherB", undefined], ["curseforge", "ActualParent", undefined],
  ]);
});

test("unresolved files and network errors remain visible instead of disappearing during deduplication", () => {
  const entries = [
    { dependency: dependency("Unavailable"), error: "network failure" },
    { dependency: dependency("", "", "manual.jar") },
  ];
  assert.deepEqual(deps.deduplicateResolvedDependencies(entries, project("Parent"), version("Own")), entries);
});

test("an exact dependency version moves to the front without mutating groups or replacing newer history", () => {
  const newer = version("Newer", "Other", ["1.21.1"]);
  const exact = version("Exact", "Other", ["1.20.1"]);
  const groups = filters.buildVersionGroups({
    versions: [newer, exact], kind: "mod", projectLoaders: ["fabric"],
    filter: filters.ALL_FILTER, groupedByDrop: false, foldOld: false,
    instanceVersion: "1.21.1", instanceLoader: "fabric",
  });
  const original = JSON.stringify(groups);
  const preferred = deps.prioritizeDependencyVersion(groups, "Exact");
  assert.equal(preferred[0].items[0].id, "Exact");
  assert.equal(JSON.stringify(groups), original);
  assert.ok(preferred.some((group) => group.items.some((item) => item.id === "Newer")));
});

test("versionless requirements never reorder history into an implicit pinned latest version", () => {
  const groups = [{ items: [version("Newer"), version("Older")] }];
  assert.deepEqual(deps.prioritizeDependencyVersion(groups), groups);
});

test("dependency detail kind is derived from the actual platform resource type", () => {
  for (const [projectType, kind] of [
    ["mod", "mod"], ["resourcepack", "resourcepack"], ["cf-class-6552", "shader"],
    ["cf-class-4471", "modpack"], ["cf-class-6945", "datapack"],
  ]) {
    assert.equal(deps.dependencyKind({ ...project("id"), projectType }), kind);
  }
});

test("resolved metadata rejects missing stable project identity, malformed versions and project/version mismatches", () => {
  assert.throws(() => parse.normalizeDependencyResolution({ project: { slug: "bad" }, version: null }, "modrinth"));
  assert.throws(() => parse.normalizeDependencyResolution({ project: project("Other"), version: {} }, "modrinth"));
  assert.throws(() => parse.normalizeDependencyResolution({ project: project("Other"), version: version("Exact", "WrongProject") }, "modrinth"));
  assert.equal(parse.normalizeDependencyResolution({ project: project("Other"), version: version("Exact", "Other") }, "modrinth").version.id, "Exact");
});

test("project-only and version-only metadata retain null and exact versions respectively", () => {
  assert.equal(parse.normalizeDependencyResolution({ project: project("Other"), version: null }, "modrinth").version, null);
  assert.equal(parse.normalizeDependencyResolution({ project: project("Other"), version: version("Exact", "Other") }, "modrinth").version.id, "Exact");
});

test("merged search keeps identical slugs and titles from different platforms or stable IDs", () => {
  const first = { ...project("A", "same-slug"), downloads: 10 };
  const second = { ...project("B", "same-slug"), downloads: 20 };
  const otherPlatform = { ...project("A", "same-slug", "curseforge"), downloads: 30 };
  assert.deepEqual(parse.mergeHits([first, second], [otherPlatform]).map((hit) => [hit.source, hit.projectId]), [
    ["curseforge", "A"], ["modrinth", "B"], ["modrinth", "A"],
  ]);
});

test("merged search collapses stable-ID aliases within one platform and falls back to slug only when ID is absent", () => {
  const original = { ...project("A", "old-slug"), downloads: 1 };
  const alias = { ...project("A", "new-slug"), downloads: 2 };
  const noId = { ...project("", "fallback"), downloads: 3 };
  assert.deepEqual(parse.mergeHits([original, noId], [alias, noId]).map((hit) => hit.slug), ["fallback", "new-slug"]);
});

test("dependency IPC reads only supplied IDs and never invokes latest, install or download commands", async () => {
  const pinned = await modrinth.resolveModrinthDependency({ versionId: "Exact" });
  const unpinned = await curseforge.resolveCurseforgeDependency({ projectId: "306612" });
  assert.equal(pinned.version.id, "Exact");
  assert.equal(unpinned.version, null);
  assert.deepEqual(calls, [
    { command: "community_dependency_resolve", args: { platform: "modrinth", projectId: null, versionId: "Exact" } },
    { command: "community_dependency_resolve", args: { platform: "curseforge", projectId: "306612", versionId: null } },
  ]);
});

test("dependency IPC failure can be retried without creating a download task", async () => {
  let failures = 1;
  invokeImpl = async () => {
    if (failures-- > 0) throw new Error("network failure");
    return JSON.stringify({ project: project("Other"), version: null });
  };
  await assert.rejects(modrinth.resolveModrinthDependency({ projectId: "Other" }), /network failure/);
  assert.equal((await modrinth.resolveModrinthDependency({ projectId: "Other" })).version, null);
  assert.ok(calls.every((call) => call.command === "community_dependency_resolve"));
});

test("ordinary saves use the existing exact-version save-as command and preserve native-dialog cancellation", async () => {
  invokeImpl = async () => null;
  const target = { kind: "mod", root: "C:/fixture", instanceVersionId: "fixture-instance", dir: "C:/fixture" };
  assert.equal(await modrinth.saveAsModrinth({ ...target, slugOrId: "Other", versionId: "Exact" }), null);
  assert.equal(await curseforge.saveAsCurseforge({ ...target, projectId: "306612", fileId: "1001" }), null);
  assert.deepEqual(calls.map((call) => [call.command, call.args.versionId ?? call.args.fileId]), [
    ["modrinth_save_as", "Exact"], ["curseforge_save_as", "1001"],
  ]);
});
