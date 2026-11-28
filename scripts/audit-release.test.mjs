import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { auditNativeBinary, auditRelease } from "./audit-release.mjs";
import { projectBuildConfig, TEST_KEY } from "./release-secrets.mjs";
import { envelope } from "../tests/embedded-config/fixtures.mjs";

const config = projectBuildConfig({
  microsoft: { clientId: "native-client-fixture" },
  curseforge: { apiKey: "native-key-fixture" },
  littleskin: { clientId: "native-skin-client", clientSecret: "native-skin-secret", redirectUri: "nickel://auth" },
});
const fields = [
  [1, config.microsoft.clientId], [2, config.microsoft.authority], [3, config.microsoft.redirectUri],
  [4, config.curseforge.apiKey], [5, config.littleskin.clientId],
  [6, config.littleskin.clientSecret], [7, config.littleskin.redirectUri],
];
const binary = (bytes) => Buffer.concat([Buffer.from("native-binary-fixture"), bytes]);

test("native audit compares field values without depending on JSON insertion order or duplicate cache copies", () => {
  const bytes = envelope([...fields].reverse());
  const report = auditNativeBinary(binary(bytes), [bytes, Buffer.from(bytes)], config);
  assert.equal(report.embeddedEnvelopeMatchesAuthorizedWhitelist, true);
  assert.equal(report.releaseConfigurationComplete, true);
  assert.equal(report.remoteAuthenticationVerified, false);
  assert.equal(report.localAttackerCannotRecoverCredentials, false);
  assert(!JSON.stringify(report).includes(config.curseforge.apiKey));
});

test("native audit rejects missing, additional, changed, corrupted and plaintext-bearing envelopes", () => {
  const bytes = envelope(fields);
  const different = envelope(fields);
  const corrupt = Buffer.from(bytes);
  corrupt[corrupt.length - 1] ^= 1;
  for (const [payload, candidates, expected] of [
    [Buffer.from("absent"), [bytes], config],
    [Buffer.concat([bytes, different]), [bytes, different], config],
    [binary(bytes), [bytes], projectBuildConfig({ ...config, curseforge: { apiKey: "changed-fixture" } })],
    [binary(corrupt), [corrupt], config],
    [Buffer.concat([bytes, Buffer.from(config.curseforge.apiKey)]), [bytes], config],
    [Buffer.concat([bytes, Buffer.from(config.microsoft.clientId, "utf16le")]), [bytes], config],
    [binary(bytes), [bytes], { ...config, tokens: ["private-fixture"] }],
  ]) assert.throws(() => auditNativeBinary(payload, candidates, expected));
  assert.throws(() => auditNativeBinary(binary(bytes), [bytes], {}));
});

test("local audit never calls an absent or placeholder credential complete", () => {
  for (const key of [null, TEST_KEY]) {
    const source = { microsoft: config.microsoft, littleskin: config.littleskin };
    if (key) source.curseforge = { apiKey: key };
    const expected = projectBuildConfig(source);
    const chosen = key ? fields.map(([id, value]) => [id, id === 4 ? key : value]) : fields.filter(([id]) => id !== 4);
    const bytes = envelope(chosen);
    assert.equal(auditNativeBinary(binary(bytes), [bytes], expected, false).releaseConfigurationComplete, false);
    assert.throws(() => auditNativeBinary(binary(bytes), [bytes], expected));
  }
});

test("actual-directory audit preserves historical files, blocks strict publication and reports local status honestly", () => {
  const root = mkdtempSync(path.join(tmpdir(), "nickel-native-audit-"));
  try {
    const release = path.join(root, "release");
    const out = path.join(release, "build", "ni-launcher-fixture", "out");
    mkdirSync(out, { recursive: true });
    const source = path.join(root, "source.json");
    writeFileSync(source, JSON.stringify(config));
    const environment = { NICKEL_BUILD_SECRETS: source };
    const bytes = envelope(fields);
    const exe = path.join(release, "ni-launcher.exe");
    writeFileSync(exe, binary(bytes));
    writeFileSync(path.join(out, "nickel-embedded-config.bin"), bytes);
    assert.equal(auditRelease(exe, release, environment).releaseReadyForPublication, true);
    const legacy = path.join(out, "nickel-embedded-secrets.json");
    writeFileSync(legacy, "historical-private-fixture");
    assert.throws(() => auditRelease(exe, release, environment), /Legacy plaintext/);
    const local = auditRelease(exe, release, environment, false);
    assert.equal(local.actualNativeExecutableScanned, true);
    assert.equal(local.legacyPlaintextBuildOutputAbsent, false);
    assert.equal(local.releaseReadyForPublication, false);
    assert.equal(readFileSync(legacy, "utf8"), "historical-private-fixture");
    assert.throws(() => auditRelease(path.join(root, "absent.exe"), release, environment), /input unavailable/);
    const evidence = path.join(root, "must-not-publish.json");
    const command = spawnSync(process.execPath, [fileURLToPath(new URL("./audit-release.mjs", import.meta.url)),
      "--binary", exe, "--release-dir", release, "--evidence", evidence], { env: { ...process.env, ...environment }, encoding: "utf8" });
    assert.notEqual(command.status, 0);
    assert.equal(existsSync(evidence), false);
    assert(!`${command.stdout}${command.stderr}`.includes("historical-private-fixture"));
    assert(!`${command.stdout}${command.stderr}`.includes(config.curseforge.apiKey));
  } finally { rmSync(root, { recursive: true }); }
});

test("audit locates envelopes in both Cargo build-dir layouts", () => {
  for (const segments of [["ni-launcher-fixture"], ["ni-launcher", "0123456789abcdef"]]) {
    const root = mkdtempSync(path.join(tmpdir(), "nickel-native-layout-"));
    try {
      const release = path.join(root, "release");
      const out = path.join(release, "build", ...segments, "out");
      mkdirSync(out, { recursive: true });
      const source = path.join(root, "source.json");
      writeFileSync(source, JSON.stringify(config));
      const bytes = envelope(fields);
      const exe = path.join(release, "ni-launcher.exe");
      writeFileSync(exe, binary(bytes));
      writeFileSync(path.join(out, "nickel-embedded-config.bin"), bytes);
      assert.equal(auditRelease(exe, release, { NICKEL_BUILD_SECRETS: source }).releaseReadyForPublication, true, segments.join("/"));
      writeFileSync(path.join(out, "nickel-embedded-secrets.json"), "historical-private-fixture");
      assert.throws(() => auditRelease(exe, release, { NICKEL_BUILD_SECRETS: source }), /Legacy plaintext/);
    } finally { rmSync(root, { recursive: true }); }
  }
});
