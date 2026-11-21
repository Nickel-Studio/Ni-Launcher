import assert from "node:assert/strict";
import test from "node:test";
import { normalizeRelease, repositoryName, siteUrl } from "./build.mjs";

const repository = "Nickel-Studio/Ni-Launcher";
const prefix = `https://github.com/${repository}/releases`;
const names = {
  "windows-exe": "Nickel_1.0.1_x64-setup.exe",
  "windows-msi": "Nickel_1.0.1_x64_en-US.msi",
  "macos-arm64": "Nickel_1.0.1_aarch64.dmg",
  "macos-x64": "Nickel_1.0.1_x64.dmg",
  "linux-appimage": "Nickel_1.0.1_amd64.AppImage",
  "linux-deb": "Nickel_1.0.1_amd64.deb",
};
function fixture() {
  return {
    tag_name: "v1.0.1",
    html_url: `${prefix}/tag/v1.0.1`,
    published_at: "2026-11-14T13:59:29Z",
    assets: Object.values(names).map((name) => ({ name, size: 1024 * 1024, browser_download_url: `${prefix}/download/v1.0.1/${name}` })),
  };
}

test("repository and Pages base URL are derived from project metadata", () => {
  assert.equal(repositoryName(`https://github.com/${repository}.git`), repository);
  assert.equal(repositoryName(`https://github.com/${repository}`), repository);
  assert.equal(siteUrl(repository), "https://nickel-studio.github.io/Ni-Launcher/");
  assert.equal(siteUrl(repository, "https://nickel.example"), "https://nickel.example/");
  assert.equal(siteUrl(repository, "https://nickel.example/site///"), "https://nickel.example/site/");
});

test("invalid repositories and insecure Pages URLs are rejected", () => {
  for (const value of ["https://example.com/a/b", "https://github.com/a", "https://github.com/a/b/c", "http://github.com/a/b"]) assert.throws(() => repositoryName(value));
  for (const value of ["http://nickel.example", "https://nickel.example/?token=x", "https://nickel.example/#x", "https://user:password@nickel.example"]) assert.throws(() => siteUrl(repository, value));
});

test("all six platform formats map to actual published files", () => {
  const release = normalizeRelease(fixture(), repository);
  assert.equal(release.tag, "v1.0.1");
  assert.equal(Object.keys(release.downloads).length, 6);
  for (const [platform, name] of Object.entries(names)) {
    assert.equal(release.downloads[platform].name, name);
    assert.equal(release.downloads[platform].url, `${prefix}/download/v1.0.1/${name}`);
    assert.equal(release.downloads[platform].size, 1024 * 1024);
  }
});

test("source archives and ARM Linux builds never become x64 downloads", () => {
  const release = fixture();
  release.assets = [
    { name: "Nickel-source.zip", browser_download_url: `${prefix}/download/v1.0.1/Nickel-source.zip` },
    { name: "Nickel_arm64.AppImage", browser_download_url: `${prefix}/download/v1.0.1/Nickel_arm64.AppImage` },
    { name: "Nickel_aarch64.deb", browser_download_url: `${prefix}/download/v1.0.1/Nickel_aarch64.deb` },
  ];
  assert.deepEqual(normalizeRelease(release, repository).downloads, {});
});

test("missing, draft, prerelease and malformed releases keep a safe fallback", () => {
  for (const value of [null, {}, { ...fixture(), draft: true }, { ...fixture(), prerelease: true }, { ...fixture(), html_url: "javascript:alert(1)" }]) {
    const result = normalizeRelease(value, repository);
    assert.equal(result.url, `${prefix}/latest`);
    assert.equal(result.tag, null);
    assert.deepEqual(result.downloads, {});
  }
});

test("download URLs cannot point outside this GitHub repository", () => {
  for (const url of ["https://evil.example/Nickel.exe", "javascript:alert(1)", "https://github.com/another/repo/releases/download/x/file.exe", "https://user:pass@github.com/Nickel-Studio/Ni-Launcher/releases/download/x/file.exe"]) {
    const release = fixture();
    release.assets[0].browser_download_url = url;
    assert.equal(normalizeRelease(release, repository).downloads["windows-exe"], undefined);
  }
});

test("missing platform formats are omitted and malformed sizes are normalized", () => {
  const release = fixture();
  release.assets = release.assets.slice(0, 1);
  release.assets[0].size = -5;
  const result = normalizeRelease(release, repository);
  assert.equal(result.downloads["windows-exe"].size, 0);
  assert.equal(result.downloads["macos-arm64"], undefined);
});
