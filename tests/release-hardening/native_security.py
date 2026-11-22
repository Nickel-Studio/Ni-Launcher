"""Test a hash-verified Windows portable ZIP using real Tauri IPC and WebView2.

Only extracts a verified archive into fresh task-owned evidence. Uses the existing
job-object/CDP ownership controls with the distribution marker instead of a modpack.
Never mocks IPC, logs credentials or accepts an EULA for a real user account.
"""
import argparse
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests/startup-acceptance"))
sys.path.insert(0, str(ROOT / "scripts"))
import native_release as native
from package_portable import MARKER, MODPACK_MARKER, marker_for, validate_archive

native.PROTECTED_PIDS.add(49092)
PIXEL = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aFuoAAAAASUVORK5CYII=")


def distribution_guard(sandbox, launching=False):
    native.validate_protocol(sandbox.protocol)
    root = native.evidence_path(sandbox.root)
    exe = native.evidence_path(sandbox.exe)
    if exe == sandbox.source or exe.parent != root or exe.name != "Nickel.exe":
        raise native.SafetyError("Only the extracted task-owned portable executable may be launched")
    if launching and native.validated_sha256(sandbox.expected_sha256, required=True) != sandbox.sha256:
        raise native.SafetyError("Explicit executable provenance required")
    if native.file_hash(exe) != sandbox.sha256:
        raise native.SafetyError("Extracted portable executable changed")
    native.assert_private_tree(root)
    marker = json.loads((root / MARKER).read_text(encoding="utf-8"))
    if marker != marker_for("x86_64-pc-windows-msvc", sandbox.protocol.version):
        raise native.SafetyError("Distribution marker changed")
    if not all((root / name).is_dir() for name in ["Nickel", ".minecraft"]):
        raise native.SafetyError("Portable data directories missing")
    if any((root / name).exists() for name in [MODPACK_MARKER, "Nickel/portable-import.json", "Nickel/pack.mrpack"]):
        raise native.SafetyError("Distribution portable must never bootstrap a modpack")
    sandbox.record_path()


def prepare(archive, expected, evidence):
    archive = native.reject_links(archive).resolve(strict=True)
    native.check(native.file_hash(archive) == native.validated_sha256(expected, required=True), "Archive hash mismatch")
    proof_name = archive.name.removesuffix(".zip") + ".json"
    proof = json.loads((archive.parent / proof_name).read_text(encoding="utf-8"))
    report = validate_archive(archive, ROOT, proof["entries"])
    native.check(report["target"] == "x86_64-pc-windows-msvc" and report["sha256"] == proof["sha256"], "Portable provenance mismatch")
    destination = native.evidence_path(evidence / "portable-extracted", new=True)
    destination.mkdir()
    with zipfile.ZipFile(archive) as stream:
        stream.extractall(destination)
    roots = list(destination.iterdir())
    native.check(len(roots) == 1 and roots[0].is_dir(), "Exactly one portable root required")
    root = roots[0]
    protocol = native.load_protocol()
    exe = root / "Nickel.exe"
    sandbox = native.Sandbox(root, exe, root / "Nickel/onboarding.json", archive, native.file_hash(exe), protocol, native.file_hash(exe))
    distribution_guard(sandbox, launching=True)
    # Synthetic pre-accepted onboarding isolates security tests from consent-flow tests.
    sandbox.seed_record({"eulaVersion": protocol.eula_revision, "eulaAcceptedAt": 1700000000, "changelogSeenVersion": protocol.version})
    settings = dict(native.SYNTHETIC_SETTINGS, accountType="microsoft", nativeSecurityFixture=True)
    (root / "Nickel/settings.json").write_text(json.dumps(settings), encoding="utf-8")
    return sandbox


class DistributionSession(native.NativeSession):
    def verify_isolation(self):
        # Fresh launcher portable archives have no modpack-generated instance registry.
        expected = self.sandbox.root / self.sandbox.protocol.game_directory
        selected = self.page.evaluate("window.__TAURI_INTERNALS__.invoke('instance_ensure_default', {})")
        native.check(isinstance(selected, str) and native.same_local_path(selected, expected),
                     "Default instance initialization escaped the extracted portable")
        super().verify_isolation()
        self.details["distributionDefaultInstanceInitialized"] = True



class PixelServer(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.send_header("Content-Length", str(len(PIXEL)))
        self.end_headers()
        self.wfile.write(PIXEL)

    def log_message(self, *_args):
        pass


def security_checks(page, fixtures, remote_url):
    valid = fixtures / "image space.png"
    valid.write_bytes(PIXEL)
    invalid = []
    for name in ["secrets.local.json", "credentials.key", "accounts.json", "active_account.json", "renamed.png", "image.svg"]:
        value = fixtures / name
        value.write_bytes(b"private-security-fixture")
        invalid.append(str(value))
    result = page.evaluate("""async ({valid, invalid, remote}) => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const convert = path => window.__TAURI_INTERNALS__.convertFileSrc(path, 'nickel-image');
      const load = src => new Promise(resolve => {
        const img = new Image(); const timer = setTimeout(() => resolve(false), 5000);
        img.onload = () => { clearTimeout(timer); resolve(img.naturalWidth > 0); };
        img.onerror = () => { clearTimeout(timer); resolve(false); }; img.src = src;
      });
      const violations = [];
      const observe = event => violations.push(event.effectiveDirective);
      document.addEventListener('securitypolicyviolation', observe);
      const script = document.createElement('script');
      script.textContent = 'window.__nickelInlineProbe = true'; document.head.append(script);
      await new Promise(resolve => setTimeout(resolve, 100)); script.remove();
      const inlineScriptBlocked = window.__nickelInlineProbe !== true && violations.includes('script-src-elem');
      let remoteFetchBlocked = false;
      try { await fetch(remote); } catch { remoteFetchBlocked = true; }
      await new Promise(resolve => setTimeout(resolve, 100));
      remoteFetchBlocked = remoteFetchBlocked && violations.includes('connect-src');
      let imageIpcUsable = false;
      try { imageIpcUsable = await load(await invoke('image_data_url', {path: valid})); } catch {}
      let privateIpcRejected = true, privateProtocolRejected = true;
      for (const path of invalid) {
        try { await invoke('image_data_url', {path}); privateIpcRejected = false; } catch {}
        if (await load(convert(path))) privateProtocolRejected = false;
      }
      let cfKeyCommandUnavailable = false;
      try { await invoke('curseforge_api_key', {}); } catch { cfKeyCommandUnavailable = true; }
      const imageProtocolUsable = await load(convert(valid));
      const remoteImageUsable = await load(remote);
      const generalAssetProtocolDisabled = !(await load(window.__TAURI_INTERNALS__.convertFileSrc(valid, 'asset')));
      document.removeEventListener('securitypolicyviolation', observe);
      return {inlineScriptBlocked, remoteFetchBlocked, imageIpcUsable, imageProtocolUsable, remoteImageUsable,
        privateIpcRejected, privateProtocolRejected, generalAssetProtocolDisabled, cfKeyCommandUnavailable};
    }""", {"valid": str(valid), "invalid": invalid, "remote": remote_url})
    native.check(isinstance(result, dict) and len(result) == 9 and all(type(value) is bool for value in result.values()), "Boolean-only native security evidence required")
    return result, valid



def device_code_ui_checks(page):
    page.get_by_role("button", name="\u4f7f\u7528 Microsoft \u767b\u5f55", exact=True).click()
    deadline = time.monotonic() + 45
    # Playwright's wait_for_function uses eval, correctly blocked by production CSP.
    # Direct CDP evaluation returns only booleans; polling never records codes or error text.
    while True:
        result = page.evaluate("""() => {
          const dialog = document.querySelector('.account-dialog');
          const error = dialog?.querySelector('[role=alert]')?.textContent || '';
          return {deviceCodeDisplayed: Boolean(dialog?.querySelector('.account-dialog__code')?.textContent?.trim()),
            dialogPresent: Boolean(dialog), failureDisplayed: Boolean(error.trim()),
            requestStillPending: Boolean(dialog?.querySelector('.account-dialog__status')),
            transportFailureReported: /connection|network|request|timed out|dns|certificate|tls/i.test(error),
            officialClientRejected: /AADSTS|invalid_client|unauthorized_client/.test(error),
            responseParsingFailed: error.includes('\u89e3\u6790\u8bbe\u5907\u4ee3\u7801\u54cd\u5e94\u5931\u8d25')};
        }""")
        native.check(isinstance(result, dict) and len(result) == 7 and all(type(value) is bool for value in result.values()),
                     "Device-code diagnostics must contain only booleans")
        if result["deviceCodeDisplayed"] or result["failureDisplayed"] or time.monotonic() >= deadline:
            return result
        page.wait_for_timeout(100)


def run(archive, expected, evidence, devicecode=False):
    evidence = native.evidence_path(evidence, new=True)
    evidence.mkdir(parents=True)
    sandbox = prepare(archive, expected, evidence)
    fixtures = evidence / "fixtures"
    fixtures.mkdir()
    reference = native.load_microsoft_reference()
    sessions, checks = [], {}
    report = {"nativeSecurityAcceptancePassed": False, "actualPortableArchiveExecuted": False,
              "sourceArchiveSha256": expected, "nativeExecutableSha256": sandbox.sha256,
              "completedAccountLoginVerified": False, "realCurseforgeRequestVerified": False}
    server = ThreadingHTTPServer(("127.0.0.1", 0), PixelServer)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    original_environment = native.isolated_environment
    system_sentinels = []
    phase = "privateNativeStartup"
    def environment(sandbox, directory, port, parent=None):
        values = original_environment(sandbox, directory, port, parent)
        system = Path(values["APPDATA"]) / "studio.nickel.launcher/settings.json"
        system.parent.mkdir(parents=True)
        system.write_text('{"accent":"rose","nativeSystemSentinel":true}', encoding="utf-8")
        system_sentinels.append((system, native.file_hash(system)))
        return values
    try:
        with native.PrivateControllerEnvironment(evidence / "controller"), \
                patch.object(native, "assert_copy", distribution_guard), \
                patch.object(native, "isolated_environment", environment):
            from playwright.sync_api import expect, sync_playwright
            with sync_playwright() as playwright:
                with DistributionSession(playwright, sandbox, evidence / "native-session", sessions) as session:
                    native.ready(session)
                    phase = "publicMicrosoftConfiguration"
                    checks["publicMicrosoftConfiguration"] = native.verify_embedded_microsoft(session.page, sandbox, reference)
                    phase = "adjacentPortableSettings"
                    persisted = session.page.evaluate("window.__TAURI_INTERNALS__.invoke('settings_get', {})")
                    values = json.loads(persisted)
                    # The frontend persists only its declared settings, removing unknown fixture fields.
                    checks["adjacentPortableSettingsSelected"] = all(values.get(key) == value for key, value in
                        {**native.SYNTHETIC_SETTINGS, "accountType": "microsoft"}.items())
                    native.check(checks["adjacentPortableSettingsSelected"], "Adjacent portable settings not selected")
                    native.check(session.page.evaluate("window.__TAURI_INTERNALS__.invoke('portable_bootstrap_status', {})") is None, "Unexpected modpack bootstrap")
                    expect(session.page.get_by_role("button", name="\u4f7f\u7528 Microsoft \u767b\u5f55", exact=True)).to_be_enabled()
                    session.page.screenshot(path=str(evidence / "portable-native-shell.png"))
                    url = f"http://127.0.0.1:{server.server_port}/pixel.png"
                    phase = "packagedCspAndImages"
                    checks["cspAndImages"], valid = security_checks(session.page, fixtures, url)
                    native.check(all(checks["cspAndImages"].values()), "Packaged native security acceptance failed")
                    phase = "persistedLocalBackground"
                    settings = json.loads(persisted)
                    settings["backgroundImage"] = str(valid)
                    session.page.evaluate("json => window.__TAURI_INTERNALS__.invoke('settings_set', {json})", json.dumps(settings))
                    session.page.reload()
                    expect(session.page.locator("#ni-splash")).to_have_count(0, timeout=15000)
                    native.ready(session)
                    session.page.wait_for_function("document.querySelector('.ni-bg__image') && getComputedStyle(document.querySelector('.ni-bg__image')).backgroundImage.includes('nickel-image')")
                    checks["persistedLocalBackgroundRendered"] = True
                    session.page.screenshot(path=str(evidence / "portable-local-background.png"))
                    if devicecode:
                        phase = "microsoftDeviceCodeUi"
                        checks["microsoftDeviceCodeUi"] = device_code_ui_checks(session.page)
                        native.check(checks["microsoftDeviceCodeUi"]["deviceCodeDisplayed"], "Native Microsoft device-code UI did not succeed")
                        checks["microsoftDeviceCodeUiAccepted"] = True
                        session.page.locator(".account-dialog .ni-modal__head button").click()
                        expect(session.page.locator(".account-dialog")).to_have_count(0)
                    phase = "systemConfigurationAndCleanup"
                    checks["systemConfigurationUnchanged"] = all(native.file_hash(path) == digest for path, digest in system_sentinels)
                    native.check(checks["systemConfigurationUnchanged"], "Portable startup changed the system configuration")
                    session.close_normally()
        report.update(nativeSecurityAcceptancePassed=True, checks=checks, sessions=sessions,
                      noModpackImport=True, allProcessesContainedBeforeResume=True,
                      allOwnedProcessesCleaned=all(not item["cleanupFailed"] and not item["forcedCleanup"] for item in sessions))
        native.check(report["allOwnedProcessesCleaned"], "Native process cleanup did not pass")
    except Exception as error:
        report.update(nativeSecurityAcceptancePassed=False, failureStage=phase,
                      failure=native.safe_failure(error), checks=checks, sessions=sessions)
    finally:
        report["actualPortableArchiveExecuted"] = any(item["nativeExecuted"] for item in sessions)
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=5)
    native.write_json(evidence / "native-security.json", report)
    print(json.dumps({"nativeSecurityAcceptancePassed": report["nativeSecurityAcceptancePassed"], "evidence": str(evidence / "native-security.json"),
                      "completedAccountLoginVerified": False, "realCurseforgeRequestVerified": False}))
    return 0 if report["nativeSecurityAcceptancePassed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--devicecode", action="store_true", help="Verify the real Microsoft device-code UI, without completing account login or recording codes")
    args = parser.parse_args()
    if os.name != "nt":
        raise native.SafetyError("This native test requires Windows")
    return run(args.archive, args.expected_sha256, args.evidence_dir, args.devicecode)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        print(json.dumps({"nativeSecurityAcceptancePassed": False, "privateValuesWithheld": True}))
        raise SystemExit(1)
