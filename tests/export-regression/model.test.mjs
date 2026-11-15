import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";
import ts from "typescript";

const source = readFileSync(new URL("../../src/components/version/vsettings/export.ts", import.meta.url), "utf8")
  .replace(/^import[^\n]*;\r?\n/gm, "");
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
});
const exports = {};
vm.runInNewContext(outputText, { exports, PRODUCT_NAME: "Nickel", TextEncoder });
const model = exports;

function directory(sourcePath, group, selected = true, displayName = sourcePath.split("/").pop()) {
  return { path: `@dir/${group}/${sourcePath}`, sourcePath, group, selected, displayName, kind: "directory", sizeBytes: null };
}
function file(path, group, selected = true) {
  return { path, sourcePath: path, displayName: path.split("/").pop(), group, selected, kind: "file", sizeBytes: 42 };
}
function preview(files) {
  return model.normalizeExportPreview({ name: "Fixture", minecraftVersion: "1.20.1", loader: null, files });
}
function paths(selection) {
  return Array.from(model.serializeExportSelection(selection));
}

const data = preview([
  directory("config", "config"), directory("config/jei/world", "jeiPersonal", false),
  file("emi.json", "emiPersonal", false), file("patchouli_data.json", "patchouliPersonal", false),
  directory("screenshots", "screenshots", false), directory("saves/World", "saves", false),
  directory("resourcepacks/Emerald", "resourcepacks", true, "Emerald Workshop"),
  directory("resourcepacks/Other", "resourcepacks", true, "Other Pack"),
  directory("journeymap", "config"), directory("journeymap", "maps", false),
]);

test("default preview selections remain compact and personal content is opt-in", () => {
  const selected = model.defaultExportSelection(data);
  assert.equal(selected.size, 4);
  assert.equal(paths(selected).length, 4);
  assert(paths(selected).every((path) => path.startsWith("@dir/")));
  for (const entry of data.files.filter((entry) => !entry.selected)) assert.equal(model.exportCheckState(entry, selected), "unchecked");
});

test("legacy file selections migrate to compact half-checked entries without widening", () => {
  const old = ["config/jei/world/server/bookmarks.ini", "screenshots/one.png", "saves/World/level.dat", "journeymap/data/sp/world/map.dat"];
  const matched = model.matchExportSelection(data, old);
  assert.equal(matched.missing, 0);
  assert.equal(matched.legacy, old.length);
  assert.deepEqual(paths(matched.selection), old);
  assert.equal(matched.selection.size, 4);
  for (const entry of data.files.filter((entry) => matched.selection.has(entry.path))) assert.equal(model.exportCheckState(entry, matched.selection), "mixed");
});

test("ten thousand legacy screenshot selections preserve exact files in one UI bucket", () => {
  const old = Array.from({ length: 10_000 }, (_, index) => `screenshots/${index}.png`);
  const matched = model.matchExportSelection(data, old);
  assert.equal(matched.selection.size, 1);
  assert.equal(matched.missing, 0);
  assert.deepEqual(paths(matched.selection), old);
});

test("checking a partial directory explicitly replaces legacy files with its token", () => {
  const partial = model.matchExportSelection(data, ["screenshots/one.png"]).selection;
  const full = model.toggleExportSelection(partial, ["@dir/screenshots/screenshots"], true);
  assert.deepEqual(paths(full), ["@dir/screenshots/screenshots"]);
  assert.equal(model.exportCheckState(data.files[4], full), "checked");
  assert.equal(model.toggleExportSelection(full, ["@dir/screenshots/screenshots"], false).size, 0);
  assert.deepEqual(paths(partial), ["screenshots/one.png"]);
});

test("whole-package selections do not include another package or its internal files", () => {
  const matched = model.matchExportSelection(data, ["@dir/resourcepacks/resourcepacks/Emerald"]);
  assert.deepEqual(paths(matched.selection), ["@dir/resourcepacks/resourcepacks/Emerald"]);
  assert.equal(matched.selection.has("@dir/resourcepacks/resourcepacks/Other"), false);
  assert.equal(data.files[6].displayName, "Emerald Workshop");
});

test("config loads separate JourneyMap settings, including .config files, from personal data", () => {
  const matched = model.matchExportSelection(data, ["journeymap/server/settings.json", "journeymap/journeymap.topo.config", "journeymap/data/sp/world/map.dat"]);
  assert.deepEqual(Array.from(matched.selection.keys()), ["@dir/config/journeymap", "@dir/maps/journeymap"]);
  assert.equal(matched.missing, 0);
  assert.deepEqual(Array.from(matched.selection.get("@dir/config/journeymap")), ["journeymap/server/settings.json", "journeymap/journeymap.topo.config"]);
});

test("directory validation rejects root packages, escapes, credentials, and wrong categories", () => {
  for (const path of ["../escape", "C:/game", "libraries/a.jar", "assets/a.json", "config/accounts.json", "scripts/probe/out.txt", "resources/playerdata/user.dat", "structures/stats/user.json", "@dir/config/config/jei/world", "@dir/jeiPersonal/config", "@dir/saves/saves/World/region", "@dir/resourcepacks/resourcepacks", "@dir/maps/libraries", "@dir/config/config/", "@dir/config/config/../other", "config/" + "x".repeat(1024), "config/" + "\u00e9".repeat(600)]) {
    assert.throws(() => model.normalizeExportConfig({ name: "Fixture", version: "1", files: [path] }), undefined, path);
  }
  assert.throws(() => model.normalizeExportConfig({ name: "Fixture", version: "1", files: ["mods/A.jar", "mods/a.jar"] }));
});

test("old configs and both export formats retain all flags but not request identity", () => {
  const old = model.normalizeExportConfig({ name: "Fixture", version: "1", files: ["screenshots/one.png"] });
  assert.equal(old.format, "mrpack");
  assert.equal(old.bundleResources, false);
  for (const format of ["mrpack", "portable"]) {
    const config = model.normalizeExportConfig({ ...old, format, includeLauncher: true, includePersonalization: true, bundleResources: true, modrinthOnly: true, dir: "private", versionId: "private" });
    assert.equal(config.includeLauncher, format === "portable");
    assert.equal(config.includePersonalization, format === "portable");
    assert.equal(config.bundleResources, true);
    assert.equal(config.modrinthOnly, true);
    assert.equal("dir" in config, false);
    assert.equal("versionId" in config, false);
    assert.deepEqual(Array.from(config.files), ["screenshots/one.png"]);
  }
});

test("preview normalization supports legacy flat files and rejects malformed selection metadata", () => {
  const old = preview([{ path: "mods/local.jar", group: "mods", selected: true, sizeBytes: 42 }]);
  assert.equal(old.files[0].displayName, "local.jar");
  assert.equal(old.files[0].kind, "file");
  assert.throws(() => preview([{ ...data.files[6], path: "@dir/resourcepacks/resourcepacks/Other" }]));
  assert.throws(() => preview([{ ...data.files[6], sizeBytes: Number.NaN }]));
  assert.throws(() => preview([data.files[6], data.files[6]]));
});


test("world-owned player files remain opt-in and legacy exact selections do not widen", () => {
  const old = ["saves/World/playerdata/user.dat", "saves/World/stats/user.json", "saves/World/advancements/user.json", "saves/World/data/cache/mod-state.dat"];
  const config = model.normalizeExportConfig({ name: "Fixture", version: "1", files: old });
  assert.deepEqual(Array.from(config.files), old);
  const matched = model.matchExportSelection(data, config.files);
  assert.equal(matched.missing, 0);
  assert.equal(matched.selection.size, 1);
  assert.equal(model.exportCheckState(data.files[5], matched.selection), "mixed");
  assert.deepEqual(paths(matched.selection), old);
  assert.equal(model.defaultExportSelection(data).has("@dir/saves/saves/World"), false);
  const whole = model.toggleExportSelection(matched.selection, ["@dir/saves/saves/World"], true);
  assert.deepEqual(paths(whole), ["@dir/saves/saves/World"]);
  assert.equal(model.exportCheckState(data.files[5], whole), "checked");
});

test("atomic pack assets and harmless token-bearing names are not globally filtered", () => {
  const old = ["resourcepacks/Token Workshop/assets/example/cache/image.png", "shaderpacks/Forest/shaders/lib/stats/util.glsl", "saves/Secret Workshop/playerdata/player.dat", "config/recipe_tokens.toml"];
  const config = model.normalizeExportConfig({ name: "Fixture", version: "1", files: old });
  assert.deepEqual(Array.from(config.files), old);
  assert.deepEqual(Array.from(model.normalizeExportConfig({ name: "Fixture", version: "1", files: ["@dir/saves/saves/Secret Workshop"] }).files), ["@dir/saves/saves/Secret Workshop"]);
});
test("resource warning has the exact approved two-line content", () => {
  assert.equal(model.BUNDLE_RESOURCES_NOTICE, "建议仅在无法稳定连接CurseForge或Modrinth 时才打包资源文件。\n公开发布含资源文件的整合包可能会违反部分Mod的使用协议。");
});

test("nineteen display categories keep disabled mods in the existing Mod category", () => {
  const expected = ["gameOptions", "personal", "mods", "config", "packData", "jeiPersonal", "emiPersonal", "patchouliPersonal", "maps", "tacz", "paintings", "resourcepacks", "shaderpacks", "shaderOptions", "schematics", "replays", "screenshots", "saves", "servers"];
  assert.deepEqual(Array.from(model.EXPORT_GROUPS, (group) => group.id), expected);
  assert.equal(model.exportDisplayGroup("disabledMods"), "mods");
  for (const id of expected) assert.equal(model.exportDisplayGroup(id), id);
});

test("disabled mod files and directory tokens preserve opt-in and exact legacy selections", () => {
  const data = preview([
    file("mods/disabled-fixture.jar.disabled", "disabledMods", false),
    directory("mods/nested", "mods", true),
    directory("mods/nested", "disabledMods", false),
  ]);
  const defaults = model.defaultExportSelection(data);
  assert.deepEqual(paths(defaults), ["@dir/mods/mods/nested"]);
  assert.equal(model.exportCheckState(data.files[0], defaults), "unchecked");
  assert.equal(model.exportCheckState(data.files[2], defaults), "unchecked");
  const old = ["mods/disabled-fixture.jar.disabled", "mods/nested/deeper.jar.disabled"];
  const config = model.normalizeExportConfig({ name: "Fixture", version: "1", files: old });
  const matched = model.matchExportSelection(data, config.files);
  assert.equal(matched.missing, 0);
  assert.equal(model.exportCheckState(data.files[2], matched.selection), "mixed");
  assert.deepEqual(paths(matched.selection), old);
  const token = model.normalizeExportConfig({ name: "Fixture", version: "1", files: ["@dir/disabledMods/mods/nested"] });
  assert.deepEqual(Array.from(token.files), ["@dir/disabledMods/mods/nested"]);
  assert.throws(() => model.normalizeExportConfig({ name: "Fixture", version: "1", files: ["@dir/disabledMods/config"] }));
});
