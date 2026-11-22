import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { literalAbsent, openEnvelope, readBuildConfig, requireReleaseConfig } from "./release-secrets.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
export function auditNativeBinary(binary, envelopes, config, requireComplete = true) {
  if (requireComplete) requireReleaseConfig(config);
  const unique = new Map(envelopes.map((bytes) => [hash(bytes), bytes]));
  const embedded = [...unique.values()].filter((bytes) => binary.includes(bytes));
  if (embedded.length !== 1 || !isDeepStrictEqual(openEnvelope(embedded[0]), config)) {
    throw new Error("Release executable does not contain exactly the authorized configuration envelope");
  }
  let complete = true;
  try { requireReleaseConfig(config); } catch { complete = false; }
  const checks = {
    actualNativeExecutableScanned: true,
    embeddedEnvelopeMatchesAuthorizedWhitelist: true,
    curseforgeLiteralAbsent: literalAbsent(binary, config.curseforge?.apiKey),
    microsoftClientLiteralAbsent: literalAbsent(binary, config.microsoft?.clientId),
    releaseConfigurationComplete: complete,
  };
  if (!checks.curseforgeLiteralAbsent || !checks.microsoftClientLiteralAbsent) {
    throw new Error("Release executable contains a plaintext configuration literal");
  }
  return { ...checks, sha256: hash(binary), envelopeSha256: hash(embedded[0]), bytes: binary.length,
    remoteAuthenticationVerified: false, localAttackerCannotRecoverCredentials: false };
}

export function auditRelease(binaryPath, releaseDirectory, environment = process.env, complete = true) {
  const config = readBuildConfig(environment);
  const outputDirectory = path.join(releaseDirectory, "build");
  const envelopes = [];
  let legacyPlaintextBuildOutputAbsent = true;
  if (!existsSync(outputDirectory) || !existsSync(binaryPath) || !statSync(binaryPath).isFile()) {
    throw new Error("Native Release audit input unavailable");
  }
  // Cargo < 1.100 uses build/ni-launcher-<hash>/out; 1.100+ nests build/ni-launcher/<hash>/... (see 9dd140c).
  const outputs = [];
  const visit = (directory, depth) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const full = path.join(directory, entry.name);
      if (entry.name === "out") outputs.push(full);
      else if (depth > 0) visit(full, depth - 1);
    }
  };
  for (const entry of readdirSync(outputDirectory, { withFileTypes: true })) {
    if (entry.isDirectory() && (entry.name === "ni-launcher" || entry.name.startsWith("ni-launcher-"))) {
      visit(path.join(outputDirectory, entry.name), 3);
    }
  }
  for (const out of outputs) {
    if (existsSync(path.join(out, "nickel-embedded-secrets.json"))) {
      legacyPlaintextBuildOutputAbsent = false;
    }
    const filename = path.join(out, "nickel-embedded-config.bin");
    if (existsSync(filename)) envelopes.push(readFileSync(filename));
  }
  if (complete && !legacyPlaintextBuildOutputAbsent) {
    throw new Error("Legacy plaintext application build output is present");
  }
  if (!envelopes.length) throw new Error("Native Release audit input unavailable");
  const audit = auditNativeBinary(readFileSync(binaryPath), envelopes, config, complete);
  return { ...audit, legacyPlaintextBuildOutputAbsent,
    releaseReadyForPublication: complete && audit.releaseConfigurationComplete && legacyPlaintextBuildOutputAbsent };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    const permitted = new Set(["--binary", "--release-dir", "--evidence", "--allow-incomplete-local"]);
    const options = {};
    for (let index = 0; index < args.length; index++) {
      if (!permitted.has(args[index]) || options[args[index]] !== undefined) throw new Error();
      if (args[index] === "--allow-incomplete-local") options[args[index]] = true;
      else {
        if (!args[index + 1] || args[index + 1].startsWith("--")) throw new Error();
        options[args[index]] = args[++index];
      }
    }
    if (!options["--binary"] || !options["--release-dir"]) throw new Error();
    const report = auditRelease(path.resolve(root, options["--binary"]), path.resolve(root, options["--release-dir"]),
      process.env, !options["--allow-incomplete-local"]);
    if (options["--evidence"]) writeFileSync(path.resolve(root, options["--evidence"]), `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" });
    console.log(JSON.stringify(report));
  } catch {
    console.error(JSON.stringify({ nativeReleaseAuditPassed: false, credentialValuesWithheld: true }));
    process.exitCode = 1;
  }
}
