import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const readJson = async (name) => JSON.parse(await readFile(resolve(root, name), "utf8"));
const [pkg, config, changelog, cargo] = await Promise.all([
  readJson("package.json"),
  readJson("src-tauri/tauri.conf.json"),
  readJson("CHANGELOG.json"),
  readFile(resolve(root, "src-tauri/Cargo.toml"), "utf8"),
]);
const cargoVersion = cargo.match(/\[package\]([\s\S]*?)(?=\n\[|$)/)?.[1].match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!pkg.version || [config.version, changelog.version, cargoVersion].some((version) => version !== pkg.version)) {
  throw new Error("Release versions must match in package.json, Cargo.toml, tauri.conf.json and CHANGELOG.json");
}
if (typeof changelog.title !== "string" || !changelog.title.trim() || !Array.isArray(changelog.sections) || !changelog.sections.length || changelog.sections.some((section) => typeof section.title !== "string" || !section.title.trim() || !Array.isArray(section.items) || !section.items.length || section.items.some((item) => typeof item !== "string" || !item.trim()))) {
  throw new Error("The built-in changelog must contain a title and nonempty release sections");
}
console.log(`Release metadata ${pkg.version}: consistent; built-in changelog validated`);
