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
vm.runInNewContext(outputText, { exports, TextEncoder });
const model = exports;

function directory(sourcePath, group, selected = true, displayName = sourcePath.split("/").pop()) {
  return { path: `@dir/${group}/${sourcePath}`, sourcePath, group, selected, displayName, kind: "directory", sizeBytes: null };
}
function file(path, group, selected = true) {
  return { path, sourcePath: path, displayName: path.split("/").pop(), group, selected, kind: "file", sizeBytes: 42 };
}
function preview(files) {
  return model.normalizeExportPreview({ name: "Fixture", minecraftVersion: "1.20.1", loader: null, files, launcher: { available: false, detail: "Not bundled", os: "windows", arch: "x86_64" }, instanceSettings: [], instanceSettingsOmitted: [], instanceSettingsError: null });
}
function config(input) {
  return model.normalizeExportConfig({ format: "mrpack", bundleResources: false, modrinthOnly: false, includeLauncher: false, includePersonalization: false, ...input });
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

test("selection matching includes only current preview identities", () => {
  const matched = model.matchExportSelection(data, ["@dir/screenshots/screenshots", "mods/unavailable.jar"]);
  assert.equal(matched.missing, 1);
  assert.deepEqual(paths(matched.selection), ["@dir/screenshots/screenshots"]);
  assert.equal(model.exportCheckState(data.files[4], matched.selection), "checked");
});



test("directory toggles change only the requested preview entry", () => {
  const selected = model.defaultExportSelection(data);
  const full = model.toggleExportSelection(selected, ["@dir/screenshots/screenshots"], true);
  assert.equal(full.size, selected.size + 1);
  assert.equal(model.exportCheckState(data.files[4], full), "checked");
  assert.deepEqual(paths(model.toggleExportSelection(full, ["@dir/screenshots/screenshots"], false)), paths(selected));
});

test("whole-package selections do not include another package or its internal files", () => {
  const matched = model.matchExportSelection(data, ["@dir/resourcepacks/resourcepacks/Emerald"]);
  assert.deepEqual(paths(matched.selection), ["@dir/resourcepacks/resourcepacks/Emerald"]);
  assert.equal(matched.selection.has("@dir/resourcepacks/resourcepacks/Other"), false);
  assert.equal(data.files[6].displayName, "Emerald Workshop");
});

test("JourneyMap configuration and personal map tokens stay separate", () => {
  const matched = model.matchExportSelection(data, ["@dir/config/journeymap", "@dir/maps/journeymap"]);
  assert.deepEqual(paths(matched.selection), ["@dir/config/journeymap", "@dir/maps/journeymap"]);
  assert.equal(matched.missing, 0);
});

test("directory validation rejects root packages, escapes, credentials, and wrong categories", () => {
  for (const path of ["../escape", "C:/game", "libraries/a.jar", "assets/a.json", "config/accounts.json", "scripts/probe/out.txt", "resources/playerdata/user.dat", "structures/stats/user.json", "@dir/config/config/jei/world", "@dir/jeiPersonal/config", "@dir/saves/saves/World/region", "@dir/resourcepacks/resourcepacks", "@dir/maps/libraries", "@dir/config/config/", "@dir/config/config/../other", "config/" + "x".repeat(1024), "config/" + "\u00e9".repeat(600)]) {
    assert.throws(() => config({ name: "Fixture", version: "1", files: [path] }), undefined, path);
  }
  assert.throws(() => config({ name: "Fixture", version: "1", files: ["mods/A.jar", "mods/a.jar"] }));
});

test("both export formats retain flags but not request identity", () => {
  for (const format of ["mrpack", "portable"]) {
    const result = config({ name: "Fixture", version: "1", files: ["@dir/screenshots/screenshots"], format, includeLauncher: true, includePersonalization: true, bundleResources: true, modrinthOnly: true, dir: "private", versionId: "private" });
    assert.equal(result.includeLauncher, format === "portable");
    assert.equal(result.includePersonalization, format === "portable");
    assert.equal(result.bundleResources, true);
    assert.equal(result.modrinthOnly, true);
    assert.equal("dir" in result, false);
    assert.equal("versionId" in result, false);
    assert.deepEqual(Array.from(result.files), ["@dir/screenshots/screenshots"]);
  }
});

test("preview normalization validates current file and directory metadata", () => {
  const result = preview([file("mods/local.jar", "mods")]);
  assert.equal(result.files[0].displayName, "local.jar");
  assert.equal(result.files[0].kind, "file");
  assert.throws(() => preview([{ ...data.files[6], path: "@dir/resourcepacks/resourcepacks/Other" }]));
  assert.throws(() => preview([{ ...data.files[6], sizeBytes: Number.NaN }]));
  assert.throws(() => preview([data.files[6], data.files[6]]));
});


test("world selection is opt-in and selects only the chosen world", () => {
  assert.equal(model.defaultExportSelection(data).has("@dir/saves/saves/World"), false);
  const matched = model.matchExportSelection(data, ["@dir/saves/saves/World"]);
  assert.equal(matched.missing, 0);
  assert.equal(matched.selection.size, 1);
  assert.deepEqual(paths(matched.selection), ["@dir/saves/saves/World"]);
  assert.equal(model.exportCheckState(data.files[5], matched.selection), "checked");
});

test("atomic package names containing token or secret are not globally filtered", () => {
  const files = ["@dir/resourcepacks/resourcepacks/Token Workshop", "@dir/shaderpacks/shaderpacks/Forest", "@dir/saves/saves/Secret Workshop"];
  assert.deepEqual(Array.from(config({ name: "Fixture", version: "1", files }).files), files);
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

test("disabled mod files and directory tokens remain opt-in", () => {
  const data = preview([
    file("mods/disabled-fixture.jar.disabled", "disabledMods", false),
    directory("mods/nested", "mods", true),
    directory("mods/nested", "disabledMods", false),
  ]);
  const defaults = model.defaultExportSelection(data);
  assert.deepEqual(paths(defaults), ["@dir/mods/mods/nested"]);
  assert.equal(model.exportCheckState(data.files[0], defaults), "unchecked");
  assert.equal(model.exportCheckState(data.files[2], defaults), "unchecked");
  const files = ["mods/disabled-fixture.jar.disabled", "@dir/disabledMods/mods/nested"];
  const matched = model.matchExportSelection(data, files);
  assert.equal(matched.missing, 0);
  assert.equal(model.exportCheckState(data.files[2], matched.selection), "checked");
  assert.deepEqual(paths(matched.selection), files);
  assert.throws(() => config({ name: "Fixture", version: "1", files: ["@dir/disabledMods/config"] }));
});
