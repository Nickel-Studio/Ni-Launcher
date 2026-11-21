import { createHash } from "node:crypto";
import { lstat, mkdir, readFile, readdir, realpath, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build, loadConfigFromFile } from "vite";

const here = fileURLToPath(new URL("./", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
const evidenceRoot = path.join(here, "evidence");
const inputs = [
  "src-tauri/src/onboarding.rs", "src/lib/onboarding.ts",
  "src/components/startup/StartupGate.tsx", "src/components/startup/startup.css",
  "src/App.tsx", "src/components/ui/Modal.tsx", "CHANGELOG.json",
  "package.json", "tsconfig.json", "vite.config.ts", "index.html",
];

async function hashes() {
  return Object.fromEntries(await Promise.all(inputs.map(async (name) => [
    name, createHash("sha256").update(await readFile(path.join(root, name))).digest("hex"),
  ])));
}

async function assertNewOutput(directory) {
  const relative = path.relative(evidenceRoot, directory);
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) {
    throw new Error("Output must be a new directory inside tests/startup-acceptance/evidence");
  }
  let current = directory;
  while (true) {
    try {
      const entry = await lstat(current);
      if (entry.isSymbolicLink() || path.relative(current, await realpath(current))) {
        throw new Error("Output ancestors must not contain symlinks or junctions");
      }
      if (current === directory) throw new Error("Output already exists; refusing to overwrite it");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    if (current === evidenceRoot) break;
    const parent = path.dirname(current);
    if (parent === current) throw new Error("Cannot locate evidence root");
    current = parent;
  }
}

async function artifacts(directory, base = directory) {
  const entries = [];
  for (const name of await readdir(directory)) {
    const absolute = path.join(directory, name);
    const info = await stat(absolute);
    if (info.isDirectory()) entries.push(...await artifacts(absolute, base));
    else entries.push({
      path: path.relative(base, absolute).replaceAll("\\", "/"),
      bytes: info.size,
      sha256: createHash("sha256").update(await readFile(absolute)).digest("hex"),
    });
  }
  return entries;
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("node tests/startup-acceptance/build_frontend.mjs --out-dir <new evidence subdirectory>");
    console.log("Builds the real frontend with its existing Vite config, without writing root dist or loading .env files.");
    return;
  }
  if (args.length !== 2 || args[0] !== "--out-dir") throw new Error("Expected --out-dir and a new evidence subdirectory");
  const outDir = path.resolve(root, args[1]);
  await assertNewOutput(outDir);
  const before = await hashes();
  const started = Date.now();
  process.env.NODE_ENV = "production";
  // Test builds must not embed ambient VITE_* values or load local secret environment files.
  for (const key of Object.keys(process.env)) if (key.startsWith("VITE_")) delete process.env[key];
  const loaded = await loadConfigFromFile(
    { command: "build", mode: "production" }, path.join(root, "vite.config.ts"), root,
    undefined, undefined, "native",
  );
  if (!loaded || path.resolve(loaded.path) !== path.join(root, "vite.config.ts")) {
    throw new Error("The production Vite configuration was not loaded");
  }
  await mkdir(path.dirname(outDir), { recursive: true });
  await build({
    ...loaded.config,
    root,
    configFile: false,
    envDir: false,
    mode: "production",
    cacheDir: path.join(path.dirname(outDir), "production-vite-cache"),
    build: { ...loaded.config.build, outDir, emptyOutDir: false, write: true },
  });
  const after = await hashes();
  const changed = inputs.filter((name) => before[name] !== after[name]);
  const files = await artifacts(outDir);
  const report = {
    scope: "real-production-frontend-build-isolated-output",
    nativeAcceptance: false,
    workspace: root,
    config: loaded.path,
    envFilesLoaded: false,
    ambientViteVariablesExcluded: true,
    outputDirectory: outDir,
    elapsedSeconds: (Date.now() - started) / 1000,
    sourceHashesBefore: before,
    sourceHashesAfter: after,
    changedInputsDuringBuild: changed,
    artifacts: files,
    passed: !changed.length && files.some((file) => file.path === "index.html"),
  };
  const reportPath = path.join(path.dirname(outDir), "production-build.json");
  await writeFile(reportPath, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  console.log("Production build evidence: " + reportPath);
  if (!report.passed) throw new Error("Production build inputs changed or index.html was not emitted");
}

main().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
