import assert from "node:assert/strict";
import { envelope } from "../tests/embedded-config/fixtures.mjs";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { literalAbsent, openEnvelope, projectBuildConfig, readBuildConfig, requireReleaseConfig, TEST_KEY, verifyRemoteConfiguration } from "./release-secrets.mjs";


const fixture = { microsoft: { clientId: "client-fixture" }, curseforge: { apiKey: "key-fixture" } };

test("whitelist defaults and environment validation match the native contract", () => {
  assert.deepEqual(projectBuildConfig({ ...fixture, tokens: ["private"], microsoft: { clientId: "client-fixture", clientSecret: "private" } }), {
    microsoft: { clientId: "client-fixture", authority: "consumers", redirectUri: "nickel://auth" }, curseforge: { apiKey: "key-fixture" },
  });
  assert.equal(projectBuildConfig(fixture, { NICKEL_CURSEFORGE_API_KEY: "  env-key  " }).curseforge.apiKey, "env-key");
  assert.equal(projectBuildConfig(fixture, { NICKEL_CURSEFORGE_API_KEY: "  " }).curseforge.apiKey, "key-fixture");
  assert.equal(projectBuildConfig(fixture, { NICKEL_CURSEFORGE_API_KEY: "bad\nvalue" }).curseforge, undefined);
  assert.equal(projectBuildConfig(fixture, { NICKEL_MSA_CLIENT_ID: "00000000-0000-0000-0000-000000000000" }).microsoft, undefined);
  assert.equal(projectBuildConfig({ microsoft: { authority: "common" } }, { NICKEL_MSA_CLIENT_ID: "env" }).microsoft.authority, "common");
});

test("release configuration fails closed for missing fields and the test placeholder", () => {
  for (const value of [{}, { microsoft: fixture.microsoft }, { ...fixture, curseforge: { apiKey: TEST_KEY } }]) {
    assert.throws(() => requireReleaseConfig(value));
  }
  assert.doesNotThrow(() => requireReleaseConfig(fixture));
});

test("source reads are bounded and never include credential values in errors", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "nickel-release-config-"));
  try {
    assert.deepEqual(readBuildConfig({}, directory), {});
    assert.throws(() => readBuildConfig({ NICKEL_BUILD_SECRETS: "absent.json" }, directory), /Unable to read/);
    const filename = path.join(directory, "source.json");
    for (const text of ['{"apiKey":"private-fixture",broken}', " ".repeat(256 * 1024 + 1)]) {
      writeFileSync(filename, text);
      assert.throws(() => readBuildConfig({ NICKEL_BUILD_SECRETS: filename }), (error) => !error.message.includes("private-fixture"));
    }
    writeFileSync(filename, JSON.stringify(fixture));
    assert.equal(readBuildConfig({ NICKEL_BUILD_SECRETS: filename }).curseforge.apiKey, "key-fixture");
  } finally {
    rmSync(directory, { recursive: true });
  }
});

test("native envelope decoder authenticates fields and rejects malformed input", () => {
  const bytes = envelope([[1, "client-fixture"], [4, "key-fixture"]]);
  assert.deepEqual(openEnvelope(bytes), fixture);
  assert(literalAbsent(bytes, "key-fixture"));
  for (let length = 0; length < bytes.length; length++) assert.throws(() => openEnvelope(bytes.subarray(0, length)));
  assert.throws(() => openEnvelope(Buffer.concat([bytes, Buffer.from([0])])));
  const corrupt = Buffer.from(bytes);
  corrupt[corrupt.length - 1] ^= 1;
  assert.throws(() => openEnvelope(corrupt), /Invalid embedded API configuration/);
  assert.throws(() => openEnvelope(envelope([[4, "x"], [4, "y"]])));
  assert.throws(() => openEnvelope(envelope([[5, "x"]])));
  assert.equal(literalAbsent(Buffer.from("key-fixture", "utf16le"), "key-fixture"), false);
});

test("remote gate uses only the official HTTPS endpoints, no redirects or private output", async () => {
  const calls = [];
  const checks = await verifyRemoteConfiguration(fixture, async (url, options) => {
    calls.push({ url, options });
    return { ok: true, json: async () => url.includes("curseforge") ? { data: { id: 432 } } :
      { device_code: "private-device", user_code: "private-code", verification_uri: "https://microsoft.com/link", expires_in: 900 } };
  });
  assert.equal(calls.length, 2);
  assert.equal(calls[0].options.redirect, "error");
  assert.equal(calls[0].options.headers["x-api-key"], "key-fixture");
  assert.equal(calls[1].options.body.get("scope"), "XboxLive.signin offline_access");
  assert(!JSON.stringify(checks).includes("private"));
  await assert.rejects(verifyRemoteConfiguration(fixture, async () => { throw new Error("key-fixture private-response"); }),
    (error) => !error.message.includes("key-fixture") && !error.message.includes("private-response"));
  await assert.rejects(verifyRemoteConfiguration(fixture, async () => ({ ok: false })), /verification failed/);
});
