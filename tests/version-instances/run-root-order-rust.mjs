/** Isolated production Rust helper tests plus optional existing crate test binary. */
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { access, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const suite = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(suite, "../..");
const args = process.argv.slice(2);
const argument = (name) => { const index = args.indexOf(name); return index < 0 ? undefined : args[index + 1]; };
const tag = argument("--tag") ?? new Date().toISOString().replace(/[:.]/g, "-");
if (!/^[a-z0-9_-]+$/i.test(tag)) throw new Error("Invalid evidence tag");
const evidence = path.join(suite, "evidence", `root-order-rust-${tag}`);
await mkdir(path.dirname(evidence), { recursive: true });
await mkdir(evidence);
const scratch = await mkdtemp(path.join(os.tmpdir(), "nickel-root-order-rust-"));
const sourceFile = path.join(root, "src-tauri/src/instances.rs");
const production = await readFile(sourceFile, "utf8");
const tests = await readFile(path.join(suite, "root-order-tests.rs"), "utf8");
const identityStart = production.indexOf("fn root_identity(");
const orderStart = production.indexOf("fn ordered_roots(", identityStart);
const end = production.indexOf("\npub(crate) fn reject_links", orderStart);
if (identityStart < 0 || orderStart < identityStart || end < orderStart) throw new Error("Production helper boundaries changed");
const extracted = production.slice(identityStart, end).trimEnd();
const sourceHash = createHash("sha256").update(production).digest("hex");
const helperHash = createHash("sha256").update(extracted).digest("hex");
const report = {
  status: "running",
  scope: "Verbatim production root_identity/ordered_roots compiled by rustc on Windows; synthetic temporary directories; no native application acceptance",
  sourceFile,
  sourceHash,
  helperHash,
  startedAt: new Date().toISOString(),
  scratch,
  stages: [],
  cleanup: {},
};

async function stage(name, command, parameters, environment = process.env) {
  const child = spawn(command, parameters, { cwd: root, shell: false, windowsHide: true, env: environment, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (value) => { stdout += value; });
  child.stderr.on("data", (value) => { stderr += value; });
  const timeout = setTimeout(() => child.kill(), 180000);
  let exitCode;
  try {
    exitCode = await new Promise((resolve, reject) => { child.on("error", reject); child.on("close", resolve); });
  } finally {
    clearTimeout(timeout);
    await writeFile(path.join(evidence, `${name}-stdout.txt`), stdout);
    await writeFile(path.join(evidence, `${name}-stderr.txt`), stderr);
  }
  const result = { name, command: [command, ...parameters], pid: child.pid, exitCode, summary: stdout.match(/test result: .*$/m)?.[0] ?? null };
  report.stages.push(result);
  if (exitCode !== 0) throw new Error(`${name} failed: ${stderr || stdout}`);
  return stdout;
}

try {
  const extractedFile = path.join(scratch, "production-root-order.rs");
  const executable = path.join(scratch, "root-order-tests.exe");
  const combined = extracted + "\n\n" + tests;
  await writeFile(extractedFile, combined);
  await writeFile(path.join(evidence, "compiled-test-source.rs"), combined);
  await stage("rustc-version", "rustc", ["--version"]);
  await stage("helper-compile", "rustc", ["--edition=2024", "--test", "--crate-name", "nickel_root_order_acceptance", extractedFile, "-o", executable]);
  await stage("helper-tests", executable, ["--test-threads=1", "--nocapture"], { ...process.env, NICKEL_ROOT_ORDER_TEMP: path.join(scratch, "fixtures") });
  const existing = argument("--existing-binary");
  if (existing) {
    const binary = path.resolve(existing);
    await access(binary);
    const metadata = await stat(binary);
    const binaryHash = createHash("sha256").update(await readFile(binary)).digest("hex");
    const copied = path.join(scratch, "existing-crate-tests.exe");
    await writeFile(copied, await readFile(binary));
    report.existingBinary = { path: binary, modifiedAt: metadata.mtime.toISOString(), sha256: binaryHash, rebuilt: false, scope: "Existing compiled crate binary copied into private temporary directory; only instances::tests::root_ selected" };
    await stage("existing-crate-root-tests", copied, ["instances::tests::root_", "--test-threads=1", "--nocapture"], { ...process.env, TEMP: scratch, TMP: scratch });
  }
  report.status = "pass";
} catch (error) {
  report.status = "fail";
  report.error = error instanceof Error ? error.stack : String(error);
} finally {
  await rm(scratch, { recursive: true, force: true });
  report.cleanup.scratchRemoved = true;
  report.finishedAt = new Date().toISOString();
  report.finalSourceHash = createHash("sha256").update(await readFile(sourceFile, "utf8")).digest("hex");
  report.productionSourceUnchanged = report.finalSourceHash === sourceHash;
  await writeFile(path.join(evidence, "result.json"), JSON.stringify(report, null, 2) + "\n");
}
console.log(JSON.stringify({ status: report.status, evidence, stages: report.stages, productionSourceUnchanged: report.productionSourceUnchanged, error: report.error, cleanup: report.cleanup }, null, 2));
process.exitCode = report.status === "pass" ? 0 : 1;
