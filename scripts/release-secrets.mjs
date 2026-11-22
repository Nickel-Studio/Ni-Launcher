import { createDecipheriv } from "node:crypto";
import { closeSync, existsSync, fstatSync, openSync, readSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const magic = Buffer.from("NICKCFG1");
export const TEST_KEY = "NICKEL_TEST_CURSEFORGE_KEY";
export const BUILD_ENVIRONMENT = [
  "NICKEL_BUILD_SECRETS", "NICKEL_MSA_CLIENT_ID", "NICKEL_MSA_AUTHORITY",
  "NICKEL_MSA_REDIRECT_URI", "NICKEL_CURSEFORGE_API_KEY", "NICKEL_REQUIRE_RELEASE_SECRETS",
];
const fields = [
  ["microsoft", "clientId"], ["microsoft", "authority"],
  ["microsoft", "redirectUri"], ["curseforge", "apiKey"],
];
const valid = (value) => typeof value === "string" && value.trim().length > 0 &&
  Buffer.byteLength(value.trim()) <= 8192 && !/\p{Cc}/u.test(value.trim());

// Mirror the Rust build whitelist; tests compare the decoded native output to this projection.
export function projectBuildConfig(source, environment = {}) {
  const merged = {};
  for (const [index, [section, name]] of fields.entries()) {
    const override = environment[BUILD_ENVIRONMENT[index + 1]];
    const value = typeof override === "string" && override.trim() ? override : source?.[section]?.[name];
    if (valid(value)) (merged[section] ??= {})[name] = value.trim();
  }
  if (!merged.microsoft?.clientId || merged.microsoft.clientId === "00000000-0000-0000-0000-000000000000") {
    delete merged.microsoft;
  } else {
    merged.microsoft.authority ??= "consumers";
    merged.microsoft.redirectUri ??= "nickel://auth";
  }
  return merged;
}

export function readBuildConfig(environment = process.env, base = root) {
  const explicit = environment.NICKEL_BUILD_SECRETS;
  const filename = explicit ? path.resolve(base, explicit) : path.join(base, "secrets.local.json");
  let source = {};
  if (existsSync(filename)) {
    let descriptor;
    let bytes;
    try {
      descriptor = openSync(filename, "r");
      const info = fstatSync(descriptor);
      const maximum = 256 * 1024;
      if (!info.isFile() || info.size > maximum) throw new Error();
      // Bound the actual read too: the file may grow after the metadata check.
      bytes = Buffer.alloc(maximum + 1);
      let count = 0;
      while (count < bytes.length) {
        const received = readSync(descriptor, bytes, count, bytes.length - count, null);
        if (!received) break;
        count += received;
      }
      if (count > maximum) throw new Error();
      source = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, count)));
    } catch {
      throw new Error("Invalid build API configuration");
    } finally {
      bytes?.fill(0);
      if (descriptor !== undefined) closeSync(descriptor);
    }
  } else if (explicit) {
    throw new Error("Unable to read build API configuration");
  }
  return projectBuildConfig(source, environment);
}

export function requireReleaseConfig(config) {
  const client = config.microsoft?.clientId;
  if (!valid(client) || client.trim() !== client || client === "00000000-0000-0000-0000-000000000000") {
    throw new Error("Release requires Microsoft public client configuration");
  }
  const key = config.curseforge?.apiKey;
  if (!valid(key) || key.trim() !== key || key === TEST_KEY) {
    throw new Error("Release requires a usable CurseForge API key");
  }
}

// A decoder for auditing the actual native build, not a claim of client-side secrecy.
export function openEnvelope(bytes) {
  const fail = () => { throw new Error("Invalid embedded API configuration"); };
  if (!Buffer.isBuffer(bytes) || bytes.length < 9 || bytes.length > 9 + 4 * (81 + 8192 + 16) ||
      !bytes.subarray(0, 8).equals(magic) || bytes[8] > 4) fail();
  let offset = 9;
  const seen = new Set();
  const records = [];
  for (let index = 0; index < bytes[8]; index++) {
    if (offset + 81 > bytes.length) fail();
    const id = bytes[offset++];
    if (id < 1 || id > 4 || seen.has(id)) fail();
    seen.add(id);
    const left = bytes.subarray(offset, offset += 32);
    const right = bytes.subarray(offset, offset += 32);
    const nonce = bytes.subarray(offset, offset += 12);
    const length = bytes.readUInt32LE(offset);
    offset += 4;
    if (length < 17 || length > 8208 || offset + length > bytes.length) fail();
    records.push({ id, left, right, nonce, ciphertext: bytes.subarray(offset, offset += length) });
  }
  if (offset !== bytes.length) fail();
  const result = {};
  for (const { id, left, right, nonce, ciphertext } of records) {
    const key = Buffer.alloc(32);
    let plaintext;
    try {
      for (let index = 0; index < key.length; index++) key[index] = left[index] ^ right[index];
      const decoder = createDecipheriv("aes-256-gcm", key, nonce);
      decoder.setAAD(Buffer.concat([magic, Buffer.from([id])]));
      decoder.setAuthTag(ciphertext.subarray(-16));
      const partial = decoder.update(ciphertext.subarray(0, -16));
      try {
        plaintext = Buffer.concat([partial, decoder.final()]);
      } finally {
        partial.fill(0);
      }
      const value = new TextDecoder("utf-8", { fatal: true }).decode(plaintext);
      if (!valid(value) || value.trim() !== value) fail();
      const [section, name] = fields[id - 1];
      (result[section] ??= {})[name] = value;
    } catch {
      fail();
    } finally {
      key.fill(0);
      plaintext?.fill(0);
    }
  }
  return result;
}

export function literalAbsent(bytes, value) {
  return !value || (!bytes.includes(Buffer.from(value, "utf8")) && !bytes.includes(Buffer.from(value, "utf16le")));
}

export async function verifyRemoteConfiguration(config, request = fetch) {
  requireReleaseConfig(config);
  const options = { redirect: "error", signal: AbortSignal.timeout(20000) };
  try {
    const response = await request("https://api.curseforge.com/v1/games/432", {
      ...options, headers: { "x-api-key": config.curseforge.apiKey, accept: "application/json" },
    });
    if (!response.ok) throw new Error();
    const result = await response.json();
    if (result.data?.id !== 432) throw new Error();
  } catch {
    throw new Error("CurseForge release credential verification failed; response details withheld");
  }
  try {
    // Use the same consumers/device-code endpoint and scope as msa.rs. Never emit device codes.
    const response = await request("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode", {
      ...options, signal: AbortSignal.timeout(20000), method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ client_id: config.microsoft.clientId, scope: "XboxLive.signin offline_access" }),
    });
    if (!response.ok) throw new Error();
    const result = await response.json();
    if (!result.device_code || !result.user_code || !result.verification_uri || !(result.expires_in > 0)) throw new Error();
  } catch {
    throw new Error("Microsoft release client verification failed; response details withheld");
  }
  return { curseforgeRemoteCredentialAccepted: true, microsoftDeviceCodeClientAccepted: true };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let phase = "releaseConfiguration";
  try {
    if (process.argv.slice(2).some((arg) => arg !== "--remote")) throw new Error();
    const config = readBuildConfig();
    requireReleaseConfig(config);
    let remote = {};
    if (process.argv.includes("--remote")) {
      phase = "remoteConfiguration";
      remote = await verifyRemoteConfiguration(config);
    }
    console.log(JSON.stringify({ releaseConfigurationComplete: true, ...remote }));
  } catch {
    console.error(JSON.stringify({ [`${phase}Passed`]: false, credentialValuesWithheld: true }));
    process.exitCode = 1;
  }
}
