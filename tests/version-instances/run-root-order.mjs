/** Isolated instance-order verification; no global frontend or native build. */
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build, preview } from "vite";
import { rootOrderConfig } from "./root-order.vite.mjs";

const suite = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(suite, "../..");
const args = process.argv.slice(2);
const argument = (name) => {
  const index = args.indexOf(name);
  return index < 0 ? undefined : args[index + 1];
};
const tag = argument("--tag") ?? new Date().toISOString().replace(/[:.]/g, "-");
if (!/^[a-z0-9_-]+$/i.test(tag)) throw new Error("Evidence tag must be a simple directory name");
const evidence = path.join(suite, "evidence", `root-order-${tag}`);
await mkdir(path.dirname(evidence), { recursive: true });
await mkdir(evidence, { recursive: false });
const scratch = await mkdtemp(path.join(os.tmpdir(), "nickel-instance-order-"));
const report = {
  status: "running",
  scope: "Production instance root UI; explicitly mocked IPC; synthetic data; isolated build; not native acceptance",
  startedAt: new Date().toISOString(),
  evidence,
  scratch,
  stages: [],
  cleanup: {},
};
let server;

async function stage(name, command, parameters, timeout = 180000) {
  const startedAt = new Date().toISOString();
  const child = spawn(command, parameters, { cwd: root, shell: false, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (data) => { stdout += data; });
  child.stderr.on("data", (data) => { stderr += data; });
  const timer = setTimeout(() => child.kill(), timeout);
  let exitCode;
  try {
    exitCode = await new Promise((resolve, reject) => {
      child.on("error", reject);
      child.on("close", resolve);
    });
  } finally {
    clearTimeout(timer);
    await writeFile(path.join(evidence, `${name}-stdout.txt`), stdout);
    await writeFile(path.join(evidence, `${name}-stderr.txt`), stderr);
  }
  const result = { name, command: [command, ...parameters], pid: child.pid, exitCode, startedAt, finishedAt: new Date().toISOString() };
  report.stages.push(result);
  if (exitCode !== 0) throw new Error(`${name} failed (${exitCode}): ${stderr || stdout}`);
  return result;
}

async function hashes() {
  const files = [
    "src/components/version/instancesApi.ts",
    "src/components/version/useInstances.ts",
    "src/components/version/VersionRootNav.tsx",
    "src/pages/Versions.tsx",
    "src-tauri/src/instances.rs",
    "tests/version-instances/root-order.tsx",
    "tests/version-instances/root-order-browser.py",
    "tests/version-instances/run-root-order.mjs",
    "tests/version-instances/root-order.vite.mjs",
    "tests/version-instances/root-order.tsconfig.json",
    "tests/version-instances/root-order.html",
  ];
  return Object.fromEntries(await Promise.all(files.map(async (file) => [file, createHash("sha256").update(await readFile(path.join(root, file))).digest("hex")])));
}

async function freePort() {
  const listener = net.createServer();
  await new Promise((resolve, reject) => { listener.once("error", reject); listener.listen(0, "127.0.0.1", resolve); });
  const port = listener.address().port;
  await new Promise((resolve, reject) => listener.close((error) => error ? reject(error) : resolve()));
  return port;
}

try {
  report.sourceHashes = await hashes();
  const tsc = fileURLToPath(new URL("./tsc.js", import.meta.resolve("typescript")));
  if (!args.includes("--skip-project-types")) {
    await stage("frontend-types", process.execPath, [tsc, "--noEmit", "--project", "tsconfig.json", "--pretty", "false"]);
  }
  await stage("fixture-types", process.execPath, [tsc, "--project", path.join(suite, "root-order.tsconfig.json"), "--pretty", "false"]);
  const buildMessages = [];
  const logger = {
    hasWarned: false,
    info: (message) => { buildMessages.push(message); },
    warn: (message) => { buildMessages.push(message); },
    warnOnce: (message) => { buildMessages.push(message); },
    error: (message) => { buildMessages.push(message); },
    clearScreen: () => {},
    hasErrorLogged: () => false,
  };
  const config = rootOrderConfig(path.join(scratch, "dist"), logger);
  const built = await build(config);
  const outputs = Array.isArray(built) ? built : [built];
  report.build = { status: "pass", output: path.join(scratch, "dist"), files: outputs.flatMap((item) => item.output?.map((entry) => ({ file: entry.fileName, type: entry.type })) ?? []) };
  await writeFile(path.join(evidence, "isolated-vite-build.txt"), buildMessages.join("\n") + "\n");
  config.preview.port = await freePort();
  server = await preview(config);
  report.base = server.resolvedUrls.local[0].replace(/\/$/, "");
  const browserArgs = [path.join(suite, "root-order-browser.py"), "--base", report.base, "--output", path.join(evidence, "browser")];
  if (argument("--only")) browserArgs.push("--only", argument("--only"));
  await stage("browser", process.env.PYTHON ?? "python", browserArgs, 240000);
  report.browser = JSON.parse(await readFile(path.join(evidence, "browser", "browser-result.json"), "utf8"));
  report.status = "pass";
} catch (error) {
  report.status = "fail";
  report.error = error instanceof Error ? error.stack : String(error);
  try { report.browser = JSON.parse(await readFile(path.join(evidence, "browser", "browser-result.json"), "utf8")); } catch { /* A pre-browser failure has no browser evidence. */ }
} finally {
  if (server) {
    await new Promise((resolve, reject) => server.httpServer.close((error) => error ? reject(error) : resolve()));
    report.cleanup.previewClosed = true;
  } else {
    report.cleanup.previewClosed = true;
  }
  await rm(scratch, { recursive: true, force: true });
  report.cleanup.scratchRemoved = true;
  report.finishedAt = new Date().toISOString();
  report.finalSourceHashes = await hashes();
  await writeFile(path.join(evidence, "result.json"), JSON.stringify(report, null, 2) + "\n");
}
console.log(JSON.stringify({ status: report.status, evidence, stages: report.stages.map(({ name, exitCode }) => ({ name, exitCode })), browser: report.browser, error: report.error, cleanup: report.cleanup }, null, 2));
process.exitCode = report.status === "pass" ? 0 : 1;
