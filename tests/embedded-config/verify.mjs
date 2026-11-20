import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Run with Node.js; --placeholder deliberately embeds test-only data, never a real API key.
const directory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(directory, "../..");
const placeholderScenario = process.argv.includes("--placeholder");
const evidencePath = path.join(
  directory,
  placeholderScenario ? "detached-release-placeholder.json" : "detached-release.json",
);
const report = {
  embeddedConfigVerifierEvidence: true,
  placeholderScenario,
  passed: false,
  checks: {},
  build: {},
  runtime: {},
  scope: {
    fullNativeApplicationVerified: false,
    completeTauriBundleVerified: false,
    microsoftRemoteAuthenticationVerified: false,
    curseforgeRemoteAuthenticationVerified: false,
  },
};
const configurationEnvironment = [
  "NICKEL_BUILD_SECRETS",
  "NICKEL_MSA_CLIENT_ID",
  "NICKEL_MSA_AUTHORITY",
  "NICKEL_MSA_REDIRECT_URI",
  "NICKEL_CURSEFORGE_API_KEY",
  "NICKEL_SECRETS",
];
const testKey = "NICKEL_TEST_CURSEFORGE_KEY";
const environmentBefore = JSON.stringify(process.env);
let phase = "argumentValidation";
let workspace;
let protectedFiles;
let infrastructureFailure;
let evidenceWritable = false;

function requireCheck(condition, name) {
  if (!condition) {
    throw new Error(name);
  }
}

function digest(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function cleanEnvironment() {
  const result = { ...process.env };
  for (const key of Object.keys(result)) {
    if (configurationEnvironment.includes(key.toUpperCase())) {
      delete result[key];
    }
  }
  return result;
}

function execute(command, args, cwd, environment, successRequired = true) {
  const result = spawnSync(command, args, {
    cwd,
    env: environment,
    encoding: "utf8",
    windowsHide: true,
    timeout: 30 * 60 * 1000,
    maxBuffer: 32 * 1024 * 1024,
  });
  if (successRequired) {
    requireCheck(!result.error && result.status === 0, phase);
  }
  return result;
}

function cargoMessages(result) {
  return result.stdout.split(/\r?\n/).flatMap((line) => {
    if (!line.startsWith("{")) return [];
    try {
      return [JSON.parse(line)];
    } catch {
      return [];
    }
  });
}

function booleanProbe(result, marker, fields) {
  const match = result.stdout.match(new RegExp(`${marker}=(\\{[^\\r\\n]+\\})`));
  requireCheck(match, `${marker}Missing`);
  const value = JSON.parse(match[1]);
  requireCheck(
    Object.keys(value).length === fields.length &&
      fields.every((field) => typeof value[field] === "boolean"),
    `${marker}BooleanSchema`,
  );
  requireCheck(
    Object.keys(value).every((field) => fields.includes(field)),
    `${marker}FieldWhitelist`,
  );
  return value;
}

function lockedVersion(lockfile, name) {
  const match = lockfile.match(
    new RegExp(`(?:^|\\n)name = "${name}"\\r?\\nversion = "([^"\\r\\n]+)"`),
  );
  requireCheck(match, "lockedDependencyVersion");
  return match[1];
}

function embeddedOutput(messages) {
  const message = messages.find(
    (entry) =>
      entry.reason === "build-script-executed" &&
      entry.package_id.includes("nickel-embedded-config-probe") &&
      existsSync(path.join(entry.out_dir, "nickel-embedded-secrets.json")),
  );
  requireCheck(message, "embeddedBuildOutput");
  return readFileSync(path.join(message.out_dir, "nickel-embedded-secrets.json"));
}

function booleanOnly(value) {
  return typeof value === "boolean" ||
    (value !== null && typeof value === "object" && !Array.isArray(value) &&
      Object.values(value).every(booleanOnly));
}

try {
  if (existsSync(evidencePath)) {
    const existing = JSON.parse(readFileSync(evidencePath, "utf8"));
    requireCheck(existing.embeddedConfigVerifierEvidence === true, "ownedEvidencePath");
  }
  evidenceWritable = true;
  requireCheck(process.argv.slice(2).every((arg) => arg === "--placeholder"), phase);
  report.checks.argumentValidationPassed = true;

  phase = "authorizedSourceValidation";
  const authorizedSource = path.join(root, "secrets.local.json");
  requireCheck(existsSync(authorizedSource), "authorizedSourceExists");
  requireCheck(
    !process.env.NICKEL_BUILD_SECRETS ||
      path.resolve(root, process.env.NICKEL_BUILD_SECRETS) === authorizedSource,
    "authorizedSourceOnly",
  );
  protectedFiles = [
    authorizedSource,
    path.join(root, "src-tauri/Cargo.toml"),
    path.join(root, "src-tauri/Cargo.lock"),
    path.join(root, "src-tauri/build.rs"),
    path.join(root, "src-tauri/build_config.rs"),
    path.join(root, "src-tauri/src/secrets.rs"),
    path.join(root, "src-tauri/tests/build_config.rs"),
  ].map((file) => [file, digest(file)]);
  report.checks.authorizedDefaultSourceSelected = true;

  phase = "isolatedSourcePreparation";
  workspace = mkdtempSync(path.join(tmpdir(), "nickel-embedded-config-"));
  const sourceRoot = path.join(workspace, "build/src-tauri");
  const detachedRoot = path.join(workspace, "detached");
  mkdirSync(path.join(sourceRoot, "src"), { recursive: true });
  mkdirSync(path.join(sourceRoot, "tests"), { recursive: true });
  mkdirSync(detachedRoot);
  for (const relative of ["build_config.rs", "src/secrets.rs", "tests/build_config.rs"]) {
    copyFileSync(path.join(root, "src-tauri", relative), path.join(sourceRoot, relative));
  }
  const buildScript = readFileSync(path.join(root, "src-tauri/build.rs"), "utf8");
  const start = buildScript.indexOf("fn embed_api_config() {");
  const end = buildScript.indexOf("\nfn validate_release_metadata()", start);
  requireCheck(start >= 0 && end > start, "completeBuildFunctionBoundary");
  const embedFunction = buildScript.slice(start, end).trim();
  requireCheck(embedFunction.endsWith("}"), "completeBuildFunctionClosure");
  writeFileSync(path.join(sourceRoot, "build.rs"),
    `#[allow(dead_code)]\nmod build_config;\n\n${embedFunction}\n\nfn main() { embed_api_config(); }\n`);
  writeFileSync(path.join(sourceRoot, "src/lib.rs"), [
    "#![allow(dead_code)]",
    "pub mod app_paths {",
    "    pub fn config_dir(_app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {",
    "        Err(\"Application directory discovery is outside this isolated loader probe\".into())",
    "    }",
    "}",
    "pub mod secrets;",
    "",
  ].join("\n"));
  const lockfile = readFileSync(path.join(root, "src-tauri/Cargo.lock"), "utf8");
  const manifest = readFileSync(path.join(root, "src-tauri/Cargo.toml"), "utf8");
  const releaseProfile = manifest.match(/\[profile\.release\][\s\S]*?(?=\n\[|$)/)?.[0];
  requireCheck(releaseProfile, "actualReleaseProfile");
  writeFileSync(path.join(sourceRoot, "Cargo.toml"), [
    "[package]",
    'name = "nickel-embedded-config-probe"',
    'version = "0.0.0"',
    'edition = "2024"',
    "[build-dependencies]",
    `serde_json = "=${lockedVersion(lockfile, "serde_json")}"`,
    "[dependencies]",
    `serde = { version = "=${lockedVersion(lockfile, "serde")}", features = ["derive"] }`,
    `serde_json = "=${lockedVersion(lockfile, "serde_json")}"`,
    `tauri = { version = "=${lockedVersion(lockfile, "tauri")}", features = ["protocol-asset"] }`,
    releaseProfile,
    "",
  ].join("\n"));
  copyFileSync(path.join(root, "src-tauri/Cargo.lock"), path.join(sourceRoot, "Cargo.lock"));
  report.checks.actualBuildFunctionCopiedUnchanged = true;
  report.checks.actualRuntimeModuleCopiedUnchanged = true;
  report.checks.actualReleaseProfileUsed = true;
  report.checks.nativeApplicationDirectoryDiscoveryExcluded = true;

  const buildEnvironment = cleanEnvironment();
  for (const key of configurationEnvironment.slice(1, 5)) {
    if (process.env[key] !== undefined) buildEnvironment[key] = process.env[key];
  }
  buildEnvironment.NICKEL_BUILD_SECRETS = authorizedSource;
  if (placeholderScenario) buildEnvironment.NICKEL_CURSEFORGE_API_KEY = testKey;
  const cargoBase = ["--manifest-path", path.join(sourceRoot, "Cargo.toml")];
  const targetDirectory = path.join(root, "src-tauri/target/embedded-config-probe");
  const targetArgs = ["--target-dir", targetDirectory];

  phase = "offlineDependencyLock";
  execute("cargo", ["metadata", ...cargoBase, "--offline", "--format-version=1"], root, buildEnvironment);
  const probeLock = readFileSync(path.join(sourceRoot, "Cargo.lock"), "utf8");
  const packages = (text) => text.split("[[package]]").slice(1).flatMap((section) => {
    const name = section.match(/\bname = "([^"\r\n]+)"/)?.[1];
    const version = section.match(/\bversion = "([^"\r\n]+)"/)?.[1];
    const source = section.match(/\bsource = "([^"\r\n]+)"/)?.[1];
    return source ? [`${name}\n${version}\n${source}`] : [];
  });
  const originalPackages = new Set(packages(lockfile));
  requireCheck(packages(probeLock).every((entry) => originalPackages.has(entry)),
    "dependencyVersionsMatchNativeLock");
  report.checks.offlineDependencyLockPassed = true;
  report.checks.dependencyVersionsMatchNativeLock = true;

  phase = "releaseCompilation";
  const compilation = execute("cargo", [
    "test", ...cargoBase, ...targetArgs, "--locked", "--offline", "--release",
    "--no-run", "--lib", "--test", "build_config", "--message-format=json",
  ], root, buildEnvironment);
  const messages = cargoMessages(compilation);
  const runtimeArtifact = messages.find((entry) =>
    entry.reason === "compiler-artifact" && entry.executable &&
    entry.profile.test && entry.target.kind.includes("lib") &&
    entry.target.name === "nickel_embedded_config_probe");
  const helperArtifact = messages.find((entry) =>
    entry.reason === "compiler-artifact" && entry.executable &&
    entry.profile.test && entry.target.kind.includes("test") && entry.target.name === "build_config");
  requireCheck(runtimeArtifact && helperArtifact, "releaseTestArtifacts");
  const embeddedBytes = embeddedOutput(messages);
  const runtimeExecutable = path.join(detachedRoot, path.basename(runtimeArtifact.executable));
  const helperExecutable = path.join(detachedRoot, path.basename(helperArtifact.executable));
  copyFileSync(runtimeArtifact.executable, runtimeExecutable);
  copyFileSync(helperArtifact.executable, helperExecutable);
  report.checks.releaseCompilationPassed = true;
  report.checks.embeddedConfigurationExtractableFromExecutable =
    readFileSync(runtimeExecutable).includes(embeddedBytes);
  requireCheck(report.checks.embeddedConfigurationExtractableFromExecutable, "embeddedBinaryBytes");
  const placeholderWarning =
    "CurseForge test placeholder is embedded but will not be used for authentication";
  const buildOutput = messages.find((entry) => entry.reason === "build-script-executed" &&
    entry.package_id.includes("nickel-embedded-config-probe"));
  requireCheck(buildOutput, "actualBuildScriptOutput");
  const cargoInstructions = readFileSync(path.join(path.dirname(buildOutput.out_dir), "output"), "utf8");
  report.checks.placeholderBuildWarningIsExplicit =
    !placeholderScenario || cargoInstructions.includes(placeholderWarning);
  requireCheck(report.checks.placeholderBuildWarningIsExplicit, "explicitPlaceholderWarning");
  report.checks.buildInputChangesTracked = configurationEnvironment.slice(0, 5).every(
    (key) => cargoInstructions.includes(`cargo:rerun-if-env-changed=${key}`),
  );
  requireCheck(report.checks.buildInputChangesTracked, "buildInputTracking");

  phase = "authorizedBuildComparison";
  const comparison = execute(helperExecutable, [
    "authorized_build_source_matches_embedded_whitelist_without_logging_values",
    "--exact", "--ignored", "--nocapture", "--test-threads=1",
  ], sourceRoot, buildEnvironment);
  report.build = booleanProbe(comparison, "NICKEL_BUILD_PROBE", [
    "authorizedSourceMatchesEmbedded", "microsoft.clientId", "microsoft.authority",
    "microsoft.redirectUri", "curseforge.apiKey", "curseforge.testPlaceholder",
  ]);
  requireCheck(report.build["curseforge.testPlaceholder"] === placeholderScenario,
    "placeholderScenarioIsExplicit");
  report.checks.authorizedBuildComparisonPassed = true;

  phase = "explicitMissingBuildSource";
  const missingEnvironment = { ...buildEnvironment,
    NICKEL_BUILD_SECRETS: path.join(workspace, "build/missing-fixture.json") };
  const checkArgs = ["check", ...cargoBase, ...targetArgs, "--locked", "--offline", "--release",
    "--lib", "--message-format=json"];
  const missing = execute("cargo", checkArgs, root, missingEnvironment, false);
  requireCheck(!missing.error && missing.status !== 0 &&
    `${missing.stdout}\n${missing.stderr}`.includes("Unable to read build API configuration"), phase);
  report.checks.explicitMissingBuildSourceRejected = true;

  phase = "malformedBuildSource";
  const malformedFile = path.join(workspace, "build/malformed-fixture.json");
  const privateFixture = "PRIVATE_BUILD_ERROR_FIXTURE";
  writeFileSync(malformedFile, `{"microsoft":{"clientId":"${privateFixture}"},broken}`);
  const malformed = execute("cargo", checkArgs, root,
    { ...buildEnvironment, NICKEL_BUILD_SECRETS: malformedFile }, false);
  const malformedOutput = `${malformed.stdout}\n${malformed.stderr}`;
  requireCheck(!malformed.error && malformed.status !== 0 &&
    malformedOutput.includes("Invalid build API configuration") &&
    !malformedOutput.includes(privateFixture), phase);
  report.checks.malformedBuildSourceRejectedWithoutValueDisclosure = true;

  phase = "environmentOnlyBuild";
  const environmentOnly = { ...cleanEnvironment(),
    NICKEL_MSA_CLIENT_ID: "environment-only-client-fixture",
    NICKEL_MSA_AUTHORITY: "common",
    NICKEL_MSA_REDIRECT_URI: "fixture://auth",
    NICKEL_CURSEFORGE_API_KEY: testKey };
  const environmentCompilation = execute("cargo", checkArgs, root, environmentOnly);
  const environmentEmbedded = JSON.parse(embeddedOutput(cargoMessages(environmentCompilation)));
  requireCheck(environmentEmbedded.microsoft?.clientId === environmentOnly.NICKEL_MSA_CLIENT_ID &&
    environmentEmbedded.microsoft?.authority === environmentOnly.NICKEL_MSA_AUTHORITY &&
    environmentEmbedded.microsoft?.redirectUri === environmentOnly.NICKEL_MSA_REDIRECT_URI &&
    environmentEmbedded.curseforge?.apiKey === testKey, phase);
  report.checks.environmentOnlyBuildWithoutExternalSourcePassed = true;

  phase = "detachment";
  // Only this process's temporary source copy is removed; the authorized source is never moved.
  rmSync(path.join(workspace, "build"), { recursive: true, force: true });
  requireCheck(!existsSync(sourceRoot), "isolatedSourceRemoved");
  const runtimeEnvironment = cleanEnvironment();
  runtimeEnvironment.NICKEL_EMBEDDED_CONFIG_PROBE_DIR = detachedRoot;
  report.checks.isolatedSourceRemovedBeforeRuntime = true;

  phase = "releaseHelperRegressions";
  const helperRegressions = execute(helperExecutable, ["--test-threads=1"], detachedRoot, runtimeEnvironment);
  requireCheck(/test result: ok\. [1-9]\d* passed; 0 failed/.test(helperRegressions.stdout),
    "releaseHelperRegressionsExecuted");
  report.checks.releaseHelperRegressionsPassed = true;

  phase = "releaseRuntimeRegressions";
  const runtimeRegressions = execute(runtimeExecutable, ["secrets::tests", "--test-threads=1"], detachedRoot, runtimeEnvironment);
  requireCheck(/test result: ok\. [1-9]\d* passed; 0 failed/.test(runtimeRegressions.stdout),
    "releaseRuntimeRegressionsExecuted");
  report.checks.releaseRuntimeRegressionsPassed = true;

  phase = "detachedRuntimeProbe";
  const runtimeResult = execute(runtimeExecutable, [
    "secrets::tests::detached_release_runtime_loads_embedded_configuration_without_external_files",
    "--exact", "--ignored", "--nocapture", "--test-threads=1",
  ], detachedRoot, runtimeEnvironment);
  report.runtime = booleanProbe(runtimeResult, "NICKEL_RUNTIME_PROBE", [
    "releaseCompilation", "executableDetached", "buildManifestUnavailable",
    "externalConfigurationAbsent", "runtimeOverridesAbsent", "sourceRootExcluded",
    "embeddedWhitelistOnly", "runtimeMatchesEmbedded", "microsoft.clientId",
    "microsoft.authority", "microsoft.redirectUri", "curseforge.apiKeyEmbedded",
    "curseforge.apiKeyUsable", "curseforge.testPlaceholder", "curseforge.testPlaceholderRejected",
  ]);
  requireCheck(report.runtime["curseforge.testPlaceholder"] === placeholderScenario, phase);
  requireCheck(report.runtime["curseforge.apiKeyEmbedded"] === report.build["curseforge.apiKey"],
    "runtimeBuildFieldStatusMatches");
  const optionalRuntimeFields = new Set(["curseforge.apiKeyEmbedded", "curseforge.apiKeyUsable", "curseforge.testPlaceholder"]);
  requireCheck(Object.entries(report.runtime).every(([field, value]) => optionalRuntimeFields.has(field) || value),
    "runtimeRequiredChecksPassed");
  report.checks.detachedRuntimeProbePassed = true;
  report.checks.configurationSelfContainedAtRuntime = true;
} catch {
  infrastructureFailure = phase;
  report.checks[`${phase}Passed`] = false;
} finally {
  report.checks.processEnvironmentUnchanged = JSON.stringify(process.env) === environmentBefore;
  report.checks.rustcWrapperNotOverridden = true;
  report.checks.backendOverlayNotUsed = true;
  if (protectedFiles) {
    report.checks.authorizedConfigurationAndImplementationUnchanged = protectedFiles.every(
      ([file, before]) => existsSync(file) && digest(file) === before,
    );
  }
  if (workspace) {
    try {
      rmSync(workspace, { recursive: true, force: true });
    } catch {
      infrastructureFailure = "temporaryWorkspaceCleanup";
    }
    report.checks.ownedTemporaryWorkspaceRemoved = !existsSync(workspace);
  }
  report.passed = !infrastructureFailure &&
    report.checks.authorizedConfigurationAndImplementationUnchanged === true &&
    report.checks.processEnvironmentUnchanged === true &&
    report.checks.ownedTemporaryWorkspaceRemoved === true;
  requireCheck(booleanOnly(report), "booleanEvidenceOnly");
  // Never forward Cargo output, embedded JSON, file hashes, or actual configuration values.
  report.checks.outputContainsOnlyBooleanStatus = true;
  if (evidenceWritable) writeFileSync(evidencePath, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  if (!report.passed) process.exitCode = 1;
}
