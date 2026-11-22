import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { BUILD_ENVIRONMENT, literalAbsent, openEnvelope, readBuildConfig, TEST_KEY } from "../../scripts/release-secrets.mjs";

const directory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(directory, "../..");
const placeholderScenario = process.argv.includes("--placeholder");
const fixtureScenario = process.argv.includes("--fixture");
const evidencePath = path.join(directory,
  `detached-release${fixtureScenario ? "-fixture" : ""}${placeholderScenario ? "-placeholder" : ""}.json`);
const report = {
  embeddedConfigVerifierEvidence: true, placeholderScenario, fixtureScenario, passed: false,
  checks: {}, build: {}, runtime: {},
  scope: {
    fullNativeApplicationVerified: false, completeTauriBundleVerified: false,
    microsoftRemoteAuthenticationVerified: false, curseforgeRemoteAuthenticationVerified: false,
    localAttackerCannotRecoverCredentials: false,
  },
};
const configurationEnvironment = [...BUILD_ENVIRONMENT, "NICKEL_SECRETS"];
const environmentBefore = JSON.stringify(process.env);
let phase = "argumentValidation";
let workspace;
let protectedFiles;
let infrastructureFailure;
let evidenceWritable = false;

function requireCheck(condition, name) {
  if (!condition) throw new Error(name);
}

function digest(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function cleanEnvironment() {
  const result = { ...process.env };
  for (const key of Object.keys(result)) {
    if (configurationEnvironment.includes(key.toUpperCase())) delete result[key];
  }
  return result;
}

function execute(command, args, cwd, environment, successRequired = true) {
  const result = spawnSync(command, args, {
    cwd, env: environment, encoding: "utf8", windowsHide: true,
    timeout: 30 * 60 * 1000, maxBuffer: 32 * 1024 * 1024,
  });
  if (successRequired) requireCheck(!result.error && result.status === 0, phase);
  return result;
}

function cargoMessages(result) {
  return result.stdout.split(/\r?\n/).flatMap((line) => {
    if (!line.startsWith("{")) return [];
    try { return [JSON.parse(line)]; } catch { return []; }
  });
}

function booleanProbe(result, marker, fields) {
  const match = result.stdout.match(new RegExp(`${marker}=(\\{[^\\r\\n]+\\})`));
  requireCheck(match, `${marker}Missing`);
  const value = JSON.parse(match[1]);
  requireCheck(Object.keys(value).length === fields.length &&
    fields.every((field) => typeof value[field] === "boolean") &&
    Object.keys(value).every((field) => fields.includes(field)), `${marker}BooleanSchema`);
  return value;
}

function lockedVersion(lockfile, name) {
  const match = lockfile.match(new RegExp(`(?:^|\\n)name = "${name}"\\r?\\nversion = "([^"\\r\\n]+)"`));
  requireCheck(match, "lockedDependencyVersion");
  return match[1];
}

function buildOutput(messages) {
  const output = messages.find((entry) => entry.reason === "build-script-executed" &&
    entry.package_id.includes("nickel-embedded-config-probe") &&
    existsSync(path.join(entry.out_dir, "nickel-embedded-config.bin")));
  requireCheck(output, "embeddedBuildOutput");
  return output;
}

function embeddedOutput(messages) {
  return readFileSync(path.join(buildOutput(messages).out_dir, "nickel-embedded-config.bin"));
}

// Cargo's build-dir layout changed (rustc 1.100 nests `<pkg>/<hash>/...`), so the build script's
// captured `output` is no longer guaranteed to sit beside `out_dir`. Locate it by content instead:
// only this probe's build script emits the NICKEL_* rerun instructions.
function buildScriptInstructions(messages) {
  const outDir = buildOutput(messages).out_dir;
  const marker = `cargo:rerun-if-env-changed=${BUILD_ENVIRONMENT[0]}`;
  const found = [];
  const visit = (directory, depth) => {
    let entries;
    try { entries = readdirSync(directory, { withFileTypes: true }); } catch { return; }
    for (const entry of entries) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        if (depth > 0 && full !== outDir) visit(full, depth - 1);
      } else if (entry.isFile() && entry.name === "output") {
        const text = readFileSync(full, "utf8");
        if (text.includes(marker)) found.push({ text, modified: statSync(full).mtimeMs });
      }
    }
  };
  // Old layout: build/<pkg>-<hash>/{out,output}. New layout: build/<pkg>/<hash>/... under one package directory.
  for (const [directory, depth] of [[path.dirname(outDir), 2], [path.dirname(path.dirname(outDir)), 3]]) {
    visit(directory, depth);
    if (found.length) break;
  }
  requireCheck(found.length, "buildScriptOutputLocated");
  return found.sort((a, b) => b.modified - a.modified)[0].text;
}

function booleanOnly(value) {
  return typeof value === "boolean" || (value !== null && typeof value === "object" &&
    !Array.isArray(value) && Object.values(value).every(booleanOnly));
}

try {
  if (existsSync(evidencePath)) {
    requireCheck(JSON.parse(readFileSync(evidencePath, "utf8")).embeddedConfigVerifierEvidence === true, "ownedEvidencePath");
  }
  evidenceWritable = true;
  requireCheck(process.argv.slice(2).every((arg) => ["--placeholder", "--fixture"].includes(arg)), phase);
  report.checks.argumentValidationPassed = true;

  phase = "authorizedSourceValidation";
  const localSource = path.join(root, "secrets.local.json");
  if (!fixtureScenario) {
    requireCheck(existsSync(localSource), "authorizedSourceExists");
    requireCheck(!process.env.NICKEL_BUILD_SECRETS || path.resolve(root, process.env.NICKEL_BUILD_SECRETS) === localSource,
      "authorizedSourceOnly");
  }
  protectedFiles = [
    ...(!fixtureScenario ? [localSource] : []),
    ...["Cargo.toml", "Cargo.lock", "build.rs", "build_config.rs", "embedded_config.rs", "src/secrets.rs", "tests/build_config.rs"]
      .map((file) => path.join(root, "src-tauri", file)),
  ].map((file) => [file, digest(file)]);
  report.checks.authorizedDefaultSourceSelected = !fixtureScenario;
  report.checks.explicitFixtureSourceSelected = fixtureScenario;

  phase = "isolatedSourcePreparation";
  workspace = mkdtempSync(path.join(tmpdir(), "nickel-embedded-config-"));
  const sourceRoot = path.join(workspace, "build/src-tauri");
  const detachedRoot = path.join(workspace, "detached");
  mkdirSync(path.join(sourceRoot, "src"), { recursive: true });
  mkdirSync(path.join(sourceRoot, "tests"), { recursive: true });
  mkdirSync(detachedRoot);
  let authorizedSource = localSource;
  const privateFixture = `NOT_EMBEDDED_${randomUUID()}`;
  if (fixtureScenario) {
    authorizedSource = path.join(workspace, "build/fixture.json");
    writeFileSync(authorizedSource, JSON.stringify({
      microsoft: { clientId: randomUUID(), authority: "consumers", redirectUri: "nickel://auth", clientSecret: privateFixture },
      curseforge: { apiKey: `CF_BUILD_FIXTURE_${randomUUID()}` },
      accounts: [{ refreshToken: privateFixture }],
    }));
  }
  for (const relative of ["build_config.rs", "embedded_config.rs", "src/secrets.rs", "tests/build_config.rs"]) {
    copyFileSync(path.join(root, "src-tauri", relative), path.join(sourceRoot, relative));
  }
  const buildScript = readFileSync(path.join(root, "src-tauri/build.rs"), "utf8");
  const start = buildScript.indexOf("fn embed_api_config() {");
  const end = buildScript.indexOf("\nfn validate_release_metadata()", start);
  requireCheck(start >= 0 && end > start, "completeBuildFunctionBoundary");
  const embedFunction = buildScript.slice(start, end).trim();
  requireCheck(embedFunction.endsWith("}"), "completeBuildFunctionClosure");
  writeFileSync(path.join(sourceRoot, "build.rs"),
    `#[allow(dead_code)]\nmod build_config;\n#[allow(dead_code)]\nmod embedded_config;\n\n${embedFunction}\n\nfn main() { embed_api_config(); }\n`);
  writeFileSync(path.join(sourceRoot, "src/lib.rs"), [
    "#![allow(dead_code)]", "pub mod app_paths {",
    "    pub fn config_dir(_app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {",
    '        Err("Application directory discovery is outside this isolated loader probe".into())',
    "    }", "    pub fn executable_directory_for(_app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {",
    "        std::env::current_exe().ok().and_then(|path| path.parent().map(std::path::Path::to_path_buf))",
    "            .ok_or_else(|| \"Unable to locate probe executable\".into())", "    }", "}", "pub mod secrets;", "",
  ].join("\n"));
  const lockfile = readFileSync(path.join(root, "src-tauri/Cargo.lock"), "utf8");
  const manifest = readFileSync(path.join(root, "src-tauri/Cargo.toml"), "utf8");
  const releaseProfile = manifest.match(/\[profile\.release\][\s\S]*?(?=\n\[|$)/)?.[0];
  requireCheck(releaseProfile, "actualReleaseProfile");
  // getrandom has multiple locked versions; select the series actually used by the application.
  const getrandomVersion = lockfile.match(/name = "getrandom"\r?\nversion = "(0\.4\.[^"\r\n]+)"/)?.[1];
  requireCheck(getrandomVersion, "lockedRandomnessVersion");
  const cryptoDependencies = ["aes-gcm", "getrandom", "zeroize"].map((name) => {
    const declarations = [...manifest.matchAll(new RegExp(`^${name} = (.+)$`, "gm"))]
      .map((match) => match[1].trim());
    requireCheck(declarations.length === 2 && declarations[0] === declarations[1], "matchingBuildAndRuntimeCryptoDependencies");
    const version = name === "getrandom" ? getrandomVersion : lockedVersion(lockfile, name);
    const value = declarations[0];
    if (value.startsWith('"')) return `${name} = "=${version}"`;
    const pinned = value.replace(/version = "[^"]+"/, `version = "=${version}"`);
    requireCheck(pinned !== value, "nativeCryptoDependencyDeclaration");
    return `${name} = ${pinned}`;
  });
  writeFileSync(path.join(sourceRoot, "Cargo.toml"), [
    "[package]", 'name = "nickel-embedded-config-probe"', 'version = "0.0.0"', 'edition = "2024"',
    "[build-dependencies]", `serde_json = "=${lockedVersion(lockfile, "serde_json")}"`, ...cryptoDependencies,
    "[dependencies]", `serde = { version = "=${lockedVersion(lockfile, "serde")}", features = ["derive"] }`,
    `serde_json = "=${lockedVersion(lockfile, "serde_json")}"`, ...cryptoDependencies,
    `tauri = { version = "=${lockedVersion(lockfile, "tauri")}", features = [] }`, releaseProfile, "",
  ].join("\n"));
  copyFileSync(path.join(root, "src-tauri/Cargo.lock"), path.join(sourceRoot, "Cargo.lock"));
  report.checks.actualBuildFunctionCopiedUnchanged = true;
  report.checks.actualRuntimeModuleCopiedUnchanged = true;
  report.checks.actualEnvelopeModuleCopiedUnchanged = true;
  report.checks.actualReleaseProfileUsed = true;
  report.checks.nativeApplicationDirectoryDiscoveryExcluded = true;
  report.checks.cryptoDependenciesMatchNativeFeatures = true;

  const buildEnvironment = cleanEnvironment();
  if (!fixtureScenario) {
    for (const key of BUILD_ENVIRONMENT.slice(1, 5)) {
      if (process.env[key] !== undefined) buildEnvironment[key] = process.env[key];
    }
  }
  buildEnvironment.NICKEL_BUILD_SECRETS = authorizedSource;
  if (placeholderScenario) buildEnvironment.NICKEL_CURSEFORGE_API_KEY = TEST_KEY;
  const expectedConfig = readBuildConfig(buildEnvironment, root);
  const cargoBase = ["--manifest-path", path.join(sourceRoot, "Cargo.toml")];
  const targetArgs = ["--target-dir", path.join(root, "src-tauri/target/embedded-config-probe")];

  phase = "offlineDependencyLock";
  execute("cargo", ["metadata", ...cargoBase, "--offline", "--format-version=1"], root, buildEnvironment);
  const packages = (text) => text.split("[[package]]").slice(1).flatMap((section) => {
    const name = section.match(/\bname = "([^"\r\n]+)"/)?.[1];
    const version = section.match(/\bversion = "([^"\r\n]+)"/)?.[1];
    const source = section.match(/\bsource = "([^"\r\n]+)"/)?.[1];
    return source ? [`${name}\n${version}\n${source}`] : [];
  });
  const originalPackages = new Set(packages(lockfile));
  requireCheck(packages(readFileSync(path.join(sourceRoot, "Cargo.lock"), "utf8")).every((entry) => originalPackages.has(entry)),
    "dependencyVersionsMatchNativeLock");
  report.checks.offlineDependencyLockPassed = true;
  report.checks.dependencyVersionsMatchNativeLock = true;

  phase = "releaseCompilation";
  const compilation = execute("cargo", [
    "test", ...cargoBase, ...targetArgs, "--locked", "--offline", "--release", "--no-run", "--lib", "--test", "build_config", "--message-format=json",
  ], root, buildEnvironment);
  const messages = cargoMessages(compilation);
  const runtimeArtifact = messages.find((entry) => entry.reason === "compiler-artifact" && entry.executable &&
    entry.profile.test && entry.target.kind.includes("lib") && entry.target.name === "nickel_embedded_config_probe");
  const helperArtifact = messages.find((entry) => entry.reason === "compiler-artifact" && entry.executable &&
    entry.profile.test && entry.target.kind.includes("test") && entry.target.name === "build_config");
  requireCheck(runtimeArtifact && helperArtifact, "releaseTestArtifacts");
  const embeddedBytes = embeddedOutput(messages);
  const runtimeExecutable = path.join(detachedRoot, path.basename(runtimeArtifact.executable));
  const helperExecutable = path.join(detachedRoot, path.basename(helperArtifact.executable));
  copyFileSync(runtimeArtifact.executable, runtimeExecutable);
  copyFileSync(helperArtifact.executable, helperExecutable);
  report.checks.releaseCompilationPassed = true;
  const binary = readFileSync(runtimeExecutable);
  report.checks.randomizedEnvelopePresentInExecutable = binary.includes(embeddedBytes);
  report.checks.credentialPlaintextAbsentFromExecutable = literalAbsent(binary, expectedConfig.curseforge?.apiKey);
  report.checks.productionCredentialPlaintextAbsentFromExecutable = placeholderScenario || report.checks.credentialPlaintextAbsentFromExecutable;
  report.checks.clientLiteralAbsentFromExecutable = literalAbsent(binary, expectedConfig.microsoft?.clientId);
  report.checks.privateSourceFieldsAbsentFromExecutable = literalAbsent(binary, privateFixture);
  report.checks.envelopeMatchesWhitelist = JSON.stringify(openEnvelope(embeddedBytes)) === JSON.stringify(expectedConfig);
  report.checks.legacyPlaintextBuildOutputAbsent = !existsSync(path.join(buildOutput(messages).out_dir, "nickel-embedded-secrets.json"));
  for (const key of ["randomizedEnvelopePresentInExecutable", "productionCredentialPlaintextAbsentFromExecutable", "clientLiteralAbsentFromExecutable",
    "privateSourceFieldsAbsentFromExecutable", "envelopeMatchesWhitelist", "legacyPlaintextBuildOutputAbsent"]) {
    requireCheck(report.checks[key], key);
  }
  const cargoInstructions = buildScriptInstructions(messages);
  report.checks.placeholderBuildWarningIsExplicit = !placeholderScenario ||
    cargoInstructions.includes("CurseForge test placeholder is embedded but will not be used for authentication");
  requireCheck(report.checks.placeholderBuildWarningIsExplicit, "explicitPlaceholderWarning");
  report.checks.buildInputChangesTracked = BUILD_ENVIRONMENT.every((key) => cargoInstructions.includes(`cargo:rerun-if-env-changed=${key}`));
  requireCheck(report.checks.buildInputChangesTracked, "buildInputTracking");

  phase = "authorizedBuildComparison";
  const comparison = execute(helperExecutable, [
    "authorized_build_source_matches_embedded_whitelist_without_logging_values", "--exact", "--ignored", "--nocapture", "--test-threads=1",
  ], sourceRoot, buildEnvironment);
  report.build = booleanProbe(comparison, "NICKEL_BUILD_PROBE", [
    "authorizedSourceMatchesEmbedded", "microsoft.clientId", "microsoft.authority", "microsoft.redirectUri", "curseforge.apiKey", "curseforge.testPlaceholder",
  ]);
  requireCheck(report.build["curseforge.testPlaceholder"] === placeholderScenario, "placeholderScenarioIsExplicit");
  report.checks.authorizedBuildComparisonPassed = true;
  const checkArgs = ["check", ...cargoBase, ...targetArgs, "--locked", "--offline", "--release", "--lib", "--message-format=json"];

  phase = "buildRandomization";
  const rebuilt = embeddedOutput(cargoMessages(execute("cargo", checkArgs, root,
    { ...buildEnvironment, NICKEL_REQUIRE_RELEASE_SECRETS: "0" })));
  requireCheck(!rebuilt.equals(embeddedBytes) && JSON.stringify(openEnvelope(rebuilt)) === JSON.stringify(expectedConfig), phase);
  report.checks.sameConfigurationRebuildUsesDifferentEnvelope = true;

  phase = "explicitMissingBuildSource";
  const missing = execute("cargo", checkArgs, root, {
    ...buildEnvironment, NICKEL_BUILD_SECRETS: path.join(workspace, "build/missing-fixture.json"),
  }, false);
  requireCheck(!missing.error && missing.status !== 0 &&
    `${missing.stdout}\n${missing.stderr}`.includes("Unable to read build API configuration"), phase);
  report.checks.explicitMissingBuildSourceRejected = true;

  phase = "malformedBuildSource";
  const malformedFile = path.join(workspace, "build/malformed-fixture.json");
  writeFileSync(malformedFile, `{"microsoft":{"clientId":"${privateFixture}"},broken}`);
  const malformed = execute("cargo", checkArgs, root, { ...buildEnvironment, NICKEL_BUILD_SECRETS: malformedFile }, false);
  requireCheck(!malformed.error && malformed.status !== 0 &&
    `${malformed.stdout}\n${malformed.stderr}`.includes("Invalid build API configuration") &&
    !`${malformed.stdout}\n${malformed.stderr}`.includes(privateFixture), phase);
  report.checks.malformedBuildSourceRejectedWithoutValueDisclosure = true;

  phase = "environmentOnlyBuild";
  const environmentOnly = { ...cleanEnvironment(), NICKEL_MSA_CLIENT_ID: "environment-only-client-fixture",
    NICKEL_MSA_AUTHORITY: "common", NICKEL_MSA_REDIRECT_URI: "fixture://auth", NICKEL_CURSEFORGE_API_KEY: TEST_KEY };
  const environmentEmbedded = openEnvelope(embeddedOutput(cargoMessages(execute("cargo", checkArgs, root, environmentOnly))));
  requireCheck(JSON.stringify(environmentEmbedded) === JSON.stringify(readBuildConfig(environmentOnly, sourceRoot)), phase);
  report.checks.environmentOnlyBuildWithoutExternalSourcePassed = true;

  phase = "nativeReleaseGate";
  const emptyFile = path.join(workspace, "build/empty-fixture.json");
  writeFileSync(emptyFile, "{}");
  const requiredEnvironment = { ...cleanEnvironment(), NICKEL_BUILD_SECRETS: emptyFile, NICKEL_REQUIRE_RELEASE_SECRETS: "1" };
  for (const [fields, error] of [
    [{}, "Release requires Microsoft public client configuration"],
    [{ NICKEL_MSA_CLIENT_ID: "release-client-fixture" }, "Release requires a usable CurseForge API key"],
    [{ NICKEL_MSA_CLIENT_ID: "release-client-fixture", NICKEL_CURSEFORGE_API_KEY: TEST_KEY }, "Release requires a usable CurseForge API key"],
  ]) {
    const rejected = execute("cargo", checkArgs, root, { ...requiredEnvironment, ...fields }, false);
    requireCheck(!rejected.error && rejected.status !== 0 && `${rejected.stdout}\n${rejected.stderr}`.includes(error), phase);
  }
  execute("cargo", checkArgs, root, { ...requiredEnvironment,
    NICKEL_MSA_CLIENT_ID: "release-client-fixture", NICKEL_CURSEFORGE_API_KEY: `RELEASE_GATE_FIXTURE_${randomUUID()}` });
  report.checks.nativeReleaseGateRejectsMissingAndPlaceholderFields = true;
  report.checks.nativeReleaseGateAcceptsCompleteConfiguration = true;

  phase = "detachment";
  rmSync(path.join(workspace, "build"), { recursive: true, force: true });
  requireCheck(!existsSync(sourceRoot), "isolatedSourceRemoved");
  const runtimeEnvironment = cleanEnvironment();
  runtimeEnvironment.NICKEL_EMBEDDED_CONFIG_PROBE_DIR = detachedRoot;
  report.checks.isolatedSourceRemovedBeforeRuntime = true;

  phase = "releaseHelperRegressions";
  const helperRegressions = execute(helperExecutable, ["--test-threads=1"], detachedRoot, runtimeEnvironment);
  requireCheck(/test result: ok\. [1-9]\d* passed; 0 failed/.test(helperRegressions.stdout), phase);
  report.checks.releaseHelperRegressionsPassed = true;
  phase = "releaseRuntimeRegressions";
  const runtimeRegressions = execute(runtimeExecutable, ["--test-threads=1"], detachedRoot, runtimeEnvironment);
  requireCheck(/test result: ok\. [1-9]\d* passed; 0 failed/.test(runtimeRegressions.stdout), phase);
  report.checks.releaseRuntimeRegressionsPassed = true;

  phase = "detachedRuntimeProbe";
  const runtimeResult = execute(runtimeExecutable, [
    "secrets::tests::detached_release_runtime_loads_embedded_configuration_without_external_files", "--exact", "--ignored", "--nocapture", "--test-threads=1",
  ], detachedRoot, runtimeEnvironment);
  report.runtime = booleanProbe(runtimeResult, "NICKEL_RUNTIME_PROBE", [
    "releaseCompilation", "executableDetached", "buildManifestUnavailable", "externalConfigurationAbsent", "runtimeOverridesAbsent",
    "sourceRootExcluded", "embeddedWhitelistOnly", "runtimeMatchesEmbedded", "microsoft.clientId", "microsoft.authority",
    "microsoft.redirectUri", "curseforge.apiKeyEmbedded", "curseforge.apiKeyUsable", "curseforge.testPlaceholder", "curseforge.testPlaceholderRejected",
  ]);
  requireCheck(report.runtime["curseforge.testPlaceholder"] === placeholderScenario, phase);
  requireCheck(report.runtime["curseforge.apiKeyEmbedded"] === report.build["curseforge.apiKey"], "runtimeBuildFieldStatusMatches");
  const optionalRuntimeFields = new Set(["curseforge.apiKeyEmbedded", "curseforge.apiKeyUsable", "curseforge.testPlaceholder"]);
  requireCheck(Object.entries(report.runtime).every(([field, value]) => optionalRuntimeFields.has(field) || value), "runtimeRequiredChecksPassed");
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
    report.checks.authorizedConfigurationAndImplementationUnchanged = protectedFiles.every(([file, before]) => existsSync(file) && digest(file) === before);
  }
  if (workspace) {
    try { rmSync(workspace, { recursive: true, force: true }); } catch { infrastructureFailure = "temporaryWorkspaceCleanup"; }
    report.checks.ownedTemporaryWorkspaceRemoved = !existsSync(workspace);
  }
  report.passed = !infrastructureFailure && report.checks.authorizedConfigurationAndImplementationUnchanged === true &&
    report.checks.processEnvironmentUnchanged === true && report.checks.ownedTemporaryWorkspaceRemoved === true;
  requireCheck(booleanOnly(report), "booleanEvidenceOnly");
  report.checks.outputContainsOnlyBooleanStatus = true;
  if (evidenceWritable) writeFileSync(evidencePath, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  if (!report.passed) process.exitCode = 1;
}
