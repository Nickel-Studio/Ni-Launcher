import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (name) => readFileSync(new URL(`../../${name}`, import.meta.url), "utf8");
const config = JSON.parse(read("src-tauri/tauri.conf.json"));

test("production CSP restricts scripts and connections without removing IPC or image compatibility", () => {
  const { csp, devCsp, assetProtocol, headers } = config.app.security;
  assert.equal(csp["default-src"], "'self'");
  for (const directive of ["base-uri", "object-src", "frame-src", "form-action", "worker-src"]) assert.equal(csp[directive], "'none'");
  assert(!/unsafe-inline|unsafe-eval|https?:|\*/.test(csp["script-src"]));
  assert.equal(csp["script-src-attr"], "'none'");
  assert.deepEqual(csp["connect-src"].split(/\s+/).sort(), ["http://ipc.localhost", "https://ipc.localhost", "ipc:"].sort());
  for (const source of ["nickel-image:", "http://nickel-image.localhost", "https://nickel-image.localhost", "data:", "blob:", "https:"]) {
    assert(csp["img-src"].split(/\s+/).includes(source));
  }
  assert(csp["style-src"].includes("'unsafe-inline'"));
  assert(devCsp["connect-src"].includes("ws:"));
  assert.deepEqual(assetProtocol, { enable: false, scope: [] });
  assert.equal(headers["X-Content-Type-Options"], "nosniff");
});

test("frontend local file conversion uses only validated image and video protocols", () => {
  const source = read("src/lib/settings.ts");
  assert(source.includes('convertFileSrc(value, "nickel-image")'));
  assert(source.includes('convertFileSrc(value, "nickel-video")'));
  assert(source.includes('invoke<string>("image_data_url"'));
  assert(read("src-tauri/src/lib.rs").includes(".plugin(media::protocol_plugin())"));
  assert(!read("src-tauri/Cargo.toml").includes('features = ["protocol-asset"]'));
  const media = read("src-tauri/src/media.rs");
  for (const forbidden of ["secrets.local.json", "credentials.key", "accounts.json", "active_account.json"]) assert(media.includes(forbidden));
  assert(media.includes('.header("x-content-type-options", "nosniff")'));
  assert(!media.includes('.header("access-control-allow-origin", "*")'));
});

test("publishing uses strict credentials, isolated uncached outputs and the all-target portable collection gate", () => {
  const workflow = read(".github/workflows/build.yml");
  assert(workflow.includes('NICKEL_REQUIRE_RELEASE_SECRETS: "1"'));
  assert(workflow.includes('cache-targets: "false"'));
  for (const target of ["x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu"]) assert(workflow.includes(target));
  assert(workflow.includes("python scripts/prepare_release.py"));
  assert(workflow.includes("python scripts/collect_release.py"));
  // The Release is uploaded from the filtered set; evidence JSON only lives in the Actions artifact.
  assert(workflow.includes("python scripts/select_release_assets.py --assets release-assets"));
  assert(workflow.includes("fs.readdirSync('release-public')"));
  assert(!workflow.includes("fs.readdirSync('release-assets')"));
  assert(workflow.includes("needs: bundle"));
  assert(workflow.includes("if-no-files-found: error"));
  assert(!workflow.includes("bundle/**"));
  const build = workflow.slice(workflow.indexOf("uses: tauri-apps/tauri-action@"), workflow.indexOf("- name: 官方"));
  assert(!/tagName:|releaseName:|releaseId:|GITHUB_TOKEN:/.test(build));
  assert(!workflow.includes("--allow-incomplete-local"));
});


test("build and runtime use the fixed clearing dependency and the probe copies their exact features", () => {
  const manifest = read("src-tauri/Cargo.toml");
  const declarations = [...manifest.matchAll(/^aes-gcm = (.+)$/gm)].map((match) => match[1].trim());
  assert.equal(declarations.length, 2);
  assert.equal(declarations[0], declarations[1]);
  assert(declarations[0].includes('version = "0.11.1"'));
  assert(declarations[0].includes('default-features = false'));
  assert(declarations[0].includes('features = ["aes", "alloc", "zeroize"]'));
  assert(read("src-tauri/embedded_config.rs").includes('require_clearing::<Aes256Gcm>'));
  const lock = read("src-tauri/Cargo.lock");
  for (const [name, series] of [["aes-gcm", "0.11"], ["aes", "0.9"], ["ghash", "0.6"], ["polyval", "0.7"]]) {
    const versions = [...lock.matchAll(new RegExp(`^name = "${name}"\\r?\\nversion = "([^"\\r\\n]+)"`, "gm"))];
    const fixed = versions.filter((match) => match[1].startsWith(`${series}.`));
    assert.equal(fixed.length, 1, `${name} must have the fixed dependency version`);
    // ZIP still depends on AES 0.8; only the GCM chain is upgraded here.
    if (name !== "aes") assert.equal(versions.length, 1, `${name} must have a single audited version`);
  }
  const probe = read("tests/embedded-config/verify.mjs");
  assert(probe.includes('manifest.matchAll(new RegExp(`^${name} = (.+)$`, "gm"))'));
  assert(probe.includes('declarations[0] === declarations[1]'));
  assert(probe.includes('cryptoDependenciesMatchNativeFeatures = true'));
});

test("media CSP allows validated video sources without opening remote fetch or generic assets", () => {
  const expected = ["'self'", "nickel-video:", "http://nickel-video.localhost", "https://nickel-video.localhost", "data:", "blob:"].sort();
  for (const csp of [config.app.security.csp, config.app.security.devCsp]) assert.deepEqual(csp["media-src"].split(/\s+/).sort(), expected);
  const media = read("src-tauri/src/media.rs");
  assert(media.includes('register_asynchronous_uri_scheme_protocol("nickel-video"'));
  assert(media.includes('MAX_VIDEO_CHUNK'));
  assert(media.includes('video_range'));
  assert(read("src-tauri/src/lib.rs").includes('media::background_media_validate'));
});
