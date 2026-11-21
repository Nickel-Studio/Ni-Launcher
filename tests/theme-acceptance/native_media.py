"""Verify local video backgrounds in a hash-authorized, isolated production WebView2.

Uses the existing native controller ownership and synthetic consent fixtures. Never
launches the source binary, copies user configuration or downloads a game.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests/startup-acceptance"))
import native_release as native

native.PROTECTED_PIDS.add(49092)
SETTINGS = "\u8bbe\u7f6e"
VIDEO_STATE = """() => {
  const videos = [...document.querySelectorAll('.ni-bg video,.st-preview video,.st-bg__thumb video')];
  return {
    threeDecodedViews: videos.length === 3 && videos.every(v => v.readyState >= 2 && v.videoWidth === 160 && v.videoHeight === 100),
    guardedLocalProtocol: videos.length === 3 && videos.every(v => /nickel-video[.:]/.test(v.currentSrc)),
    mutedLoopInline: videos.length === 3 && videos.every(v => v.muted && v.loop && v.playsInline && !v.controls),
    coverRendering: videos.length === 3 && videos.every(v => getComputedStyle(v).objectFit === 'cover'),
    themeBackgroundExcluded: document.querySelectorAll('.ni-bg .ni-theme-artwork,.st-preview__art .ni-theme-artwork').length === 0,
    titlebarMotifPreserved: document.querySelectorAll('.ni-titlebar > .ni-theme-artwork').length === 1,
    thumbnailPaused: Boolean(document.querySelector('.st-bg__thumb video')?.paused),
    shellPlaying: Boolean(document.querySelector('.ni-bg video') && !document.querySelector('.ni-bg video').paused),
    liveBlurAndOpacity: Boolean(document.querySelector('.ni-bg video') && getComputedStyle(document.querySelector('.ni-bg video')).filter === 'blur(8px)' && Number(getComputedStyle(document.querySelector('.ni-bg video')).opacity) === 0.7)
  };
}"""


def wait_for(page, predicate, timeout=15):
    # Playwright wait_for_function uses dynamic eval, which production CSP rejects.
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if page.evaluate(predicate) is True:
            return
        page.wait_for_timeout(100)
    raise native.NativeAssertion("Native media did not reach the required state")


def ready(session):
    from playwright.sync_api import expect
    wait_for(session.page, "Boolean(document.querySelector('.ni-shell') && !document.querySelector('.ni-shell').inert)")
    expect(session.page.locator(".startup-modal")).to_have_count(0)
    session.assert_live()


def flags(value, expected_count):
    native.check(isinstance(value, dict) and len(value) == expected_count and
                 all(type(flag) is bool for flag in value.values()),
                 "Native media evidence must contain only booleans")
    return value


def run(source, expected, evidence):
    evidence = native.evidence_path(evidence, new=True)
    evidence.mkdir(parents=True)
    source = native.executable_source(source)
    native.check(native.file_hash(source) == native.validated_sha256(expected, required=True), "Executable hash mismatch")
    sandbox = native.prepare_copy(source, evidence / "portable", expected_sha256=expected)
    sandbox.seed_record({"eulaVersion": sandbox.protocol.eula_revision, "eulaAcceptedAt": 1700000000,
                         "changelogSeenVersion": sandbox.protocol.version})
    fixtures = evidence / "fixtures"
    fixtures.mkdir()
    video = fixtures / "background space \u80cc\u666f.webm"
    shutil.copyfile(ROOT / "tests/theme-acceptance/fixtures/background.webm", video)
    invalid = []
    for name in ["secrets.local.json", "credentials.key", "accounts.json", "renamed.webm", "renamed.mp4", "image.svg"]:
        file = fixtures / name
        file.write_bytes(b"private-native-media-fixture")
        invalid.append(str(file))
    sessions, checks = [], {}
    report = {"nativeMediaAcceptancePassed": False, "sourceExecutableSha256": expected,
              "syntheticConsentOnly": True, "realAuthenticationExercised": False}
    phase = "isolatedNativeStartup"
    try:
        with native.PrivateControllerEnvironment(evidence / "controller"):
            from playwright.sync_api import sync_playwright
            with sync_playwright() as playwright:
                with native.NativeSession(playwright, sandbox, evidence / "session", sessions) as session:
                    ready(session)
                    page = session.page
                    phase = "nativeMediaValidation"
                    checks["pathValidation"] = flags(page.evaluate("""async ({valid, invalid}) => {
                      const invoke = window.__TAURI_INTERNALS__.invoke;
                      let validAccepted = false, invalidRejected = true, valuesWithheld = true;
                      try { await invoke('background_media_validate', {path: valid}); validAccepted = true; } catch {}
                      for (const path of invalid) {
                        try { await invoke('background_media_validate', {path}); invalidRejected = false; }
                        catch (error) { if (String(error).includes('private-native-media-fixture') || String(error).includes(path)) valuesWithheld = false; }
                      }
                      return {validAccepted, invalidRejected, valuesWithheld};
                    }""", {"valid": str(video), "invalid": invalid}), 3)
                    native.check(all(checks["pathValidation"].values()), "Native media validation failed")
                    phase = "localVideoPersistenceAndDecoding"
                    settings = json.loads(page.evaluate("window.__TAURI_INTERNALS__.invoke('settings_get', {})"))
                    settings.update(backgroundImage=str(video), backgroundBlur=8, backgroundOpacity=0.7)
                    page.evaluate("json => window.__TAURI_INTERNALS__.invoke('settings_set', {json})", json.dumps(settings))
                    page.reload()
                    ready(session)
                    page.get_by_role("navigation", name="\u4e3b\u5bfc\u822a").get_by_role("button", name=SETTINGS, exact=True).click()
                    wait_for(page, "Boolean(document.querySelector('.st-bg__thumb video'))")
                    wait_for(page, "[...document.querySelectorAll('.ni-bg video,.st-preview video,.st-bg__thumb video')].length === 3 && [...document.querySelectorAll('.ni-bg video,.st-preview video,.st-bg__thumb video')].every(v => v.readyState >= 2)")
                    checks["decodedLocalVideo"] = flags(page.evaluate(VIDEO_STATE), 9)
                    native.check(all(checks["decodedLocalVideo"].values()), "Production WebView2 video rendering failed")
                    wait_for(page, "document.querySelector('.ni-bg video').currentTime > 0.08")
                    checks["localPathPersistedWithoutBase64"] = json.loads(page.evaluate("window.__TAURI_INTERNALS__.invoke('settings_get', {})")).get("backgroundImage") == str(video)
                    native.check(checks["localPathPersistedWithoutBase64"], "Native background persistence changed the local media path")
                    phase = "nativeSeekAndReducedMotion"
                    page.evaluate("document.querySelector('.ni-bg video').currentTime = 0.6")
                    wait_for(page, "document.querySelector('.ni-bg video').currentTime >= 0.6 && document.querySelector('.ni-bg video').readyState >= 2")
                    checks["nativeVideoSeekWorks"] = True
                    page.emulate_media(reduced_motion="reduce")
                    wait_for(page, "[...document.querySelectorAll('video')].every(v => v.paused && v.readyState >= 2)")
                    checks["reducedMotionPreservesDecodedFrames"] = True
                    page.emulate_media(reduced_motion="no-preference")
                    wait_for(page, "!document.querySelector('.ni-bg video').paused")
                    checks["motionPreferenceResumesVideo"] = True
                    page.screenshot(path=str(evidence / "native-video-background.png"), full_page=True)
                    phase = "normalNativeExit"
                    session.close_normally()
        checks["ownedProcessesCleaned"] = bool(sessions) and all(not item["cleanupFailed"] and not item["forcedCleanup"] and item.get("processExit", {}).get("exitCode") == 0 for item in sessions)
        native.check(checks["ownedProcessesCleaned"], "Native media session did not exit and clean up normally")
        report.update(nativeMediaAcceptancePassed=True, checks=checks, sessions=sessions)
    except Exception as error:
        report.update(nativeMediaAcceptancePassed=False, failureStage=phase, failure=native.safe_failure(error), checks=checks, sessions=sessions)
    native.write_json(evidence / "native-media.json", report)
    print(json.dumps({"nativeMediaAcceptancePassed": report["nativeMediaAcceptancePassed"], "evidence": str(evidence / "native-media.json")}))
    return 0 if report["nativeMediaAcceptancePassed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    args = parser.parse_args()
    native.check(os.name == "nt", "Native media acceptance requires Windows")
    return run(args.exe, args.expected_sha256, args.evidence_dir)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        print(json.dumps({"nativeMediaAcceptancePassed": False, "privateValuesWithheld": True}))
        raise SystemExit(1)
