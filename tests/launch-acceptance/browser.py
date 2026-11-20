"""Exercise production launch components in a real browser with simulated IPC."""
import argparse
import base64
import json
import re
import traceback
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

START = "\u542f\u52a8\u6e38\u620f"
CANCEL = "\u53d6\u6d88\u542f\u52a8"
EXPORT = "\u5bfc\u51fa\u9519\u8bef\u62a5\u544a ZIP"
BUSY = "\u6b63\u5728\u5bfc\u51fa\u5b8c\u6574\u62a5\u544a..."
KNOWN = "\u77e5\u9053\u4e86"
LOGS = "\u6253\u5f00\u65e5\u5fd7\u76ee\u5f55"
SUCCESS = "\u5b8c\u6574\u62a5\u544a\u5df2\u5bfc\u51fa"
FAILED = "\u62a5\u544a\u672a\u80fd\u5bfc\u51fa"
FIXTURE = "window.__launchFixture"
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jA2sAAAAASUVORK5CYII=")


def snapshot(page):
    return page.evaluate(FIXTURE + ".snapshot()")


def state(page):
    return snapshot(page)["state"]


def calls(page, command):
    return [call for call in snapshot(page)["calls"] if call["command"] == command]


def event(page, payload):
    page.evaluate("payload => window.__launchFixture.event(payload)", payload)
    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")


def launch(page, patch=None):
    page.evaluate("patch => window.__launchFixture.launch(patch)", patch or {})
    page.wait_for_function("window.__launchFixture.snapshot().state.starting === true")


GEOMETRY = """() => {
  const rect = element => {
    const box = element.getBoundingClientRect();
    return { x: box.x, y: box.y, width: box.width, height: box.height, bottom: box.bottom, right: box.right };
  };
  const style = element => {
    const value = getComputedStyle(element);
    return { opacity: Number(value.opacity), transform: value.transform, visibility: value.visibility, transition: value.transitionDuration };
  };
  const card = document.querySelector('.home__card');
  const presentation = document.querySelector('.home__presentation');
  const account = document.querySelector('.home__account');
  const progress = document.querySelector('.home__launch-progress');
  const content = document.querySelector('.ni-content');
  return {
    card: rect(card), presentation: rect(presentation), account: style(account), progress: style(progress),
    progressBox: rect(progress), button: rect(document.querySelector('.launch-button')),
    starting: presentation.dataset.starting,
    accountInert: account.inert, progressInert: progress.inert,
    contentOverflow: { x: content.scrollWidth - content.clientWidth, y: content.scrollHeight - content.clientHeight },
    cardOverflow: card.scrollWidth - card.clientWidth,
  };
}"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--channel", default="msedge")
    parser.add_argument("--smoke", action="store_true", help="Run only the essential launch paths")
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    results = []
    checks = []

    def check(name, condition, details=None):
        checks.append({"name": name, "passed": bool(condition), "details": details})
        if not condition:
            raise AssertionError(name + (": " + str(details) if details is not None else ""))

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel=args.channel, headless=True)

        smoke_cases = {
            "home-1100-720-1-no-preference", "hook-repeat-and-late-progress",
            "hook-cancel-failure-retry", "hook-previous-session-late-events",
            "hook-listener-failure-fails-closed", "home-failed-instance-report-snapshot",
        }

        def run_case(name, setup, test, width=1100, height=720, scale=1, reduced="no-preference"):
            if args.smoke and name not in smoke_cases:
                return
            output = args.evidence_dir / name
            output.mkdir()
            first_check = len(checks)
            context = browser.new_context(viewport={"width": width, "height": height}, device_scale_factor=scale, reduced_motion=reduced)
            page = context.new_page()
            errors = []
            stubs = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.on("console", lambda message: errors.append(message.text) if message.type == "error" else None)

            def route(request):
                if urlsplit(request.request.url).hostname in {"127.0.0.1", "localhost"}:
                    request.continue_()
                else:
                    stubs.append(request.request.url)
                    request.fulfill(status=200, body=PNG, content_type="image/png")

            context.route("**/*", route)
            page.add_init_script("window.__LAUNCH_SETUP__ = " + json.dumps(setup, ensure_ascii=True) + "; window.__unhandled = []; window.addEventListener('unhandledrejection', event => window.__unhandled.push(String(event.reason)));")
            error = None
            try:
                page.goto(args.url, wait_until="networkidle")
                page.wait_for_function("Boolean(window.__launchFixture)")
                if setup.get("mode") == "hook":
                    page.wait_for_function("window.__launchFixture.snapshot().state !== null")
                elif setup.get("mode") == "dialog":
                    expect(page.get_by_role("dialog")).to_be_visible()
                else:
                    expect(page.locator(".home__name")).to_have_text("AcceptancePlayer")
                    expect(page.locator(".launch-button")).to_be_enabled()
                    page.evaluate("() => Promise.all(document.getAnimations().filter(animation => animation.effect?.getTiming().iterations !== Infinity).map(animation => animation.finished.catch(() => {})))")
                test(page, output)
                check(name + ": no browser errors", not errors, errors)
                check(name + ": no unhandled rejection", not page.evaluate("window.__unhandled"), page.evaluate("window.__unhandled"))
            except Exception as cause:
                error = str(cause)
                (output / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            finally:
                try:
                    page.screenshot(path=str(output / "screen.png"), full_page=True)
                    data = snapshot(page)
                except Exception:
                    data = None
                result = {"name": name, "passed": error is None, "error": error, "viewport": {"width": width, "height": height, "scale": scale, "reducedMotion": reduced}, "checks": checks[first_check:], "snapshot": data, "browserErrors": errors, "remoteRequestsStubbed": stubs}
                (output / "result.json").write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
                results.append(result)
                context.close()
                print(("PASS " if error is None else "FAIL ") + name + (": " + error if error else ""), flush=True)

        def home_geometry(page, output):
            baseline = page.evaluate(GEOMETRY)
            page.evaluate("window.__geometryFrames = []")
            frames = page.evaluate("""async geometrySource => {
              const geometry = (0, eval)('(' + geometrySource + ')');
              const started = performance.now();
              document.querySelector('.launch-button').click();
              const frames = [];
              await new Promise(resolve => {
                const sample = () => {
                  frames.push({ elapsed: performance.now() - started, ...geometry() });
                  if (performance.now() - started < 460) requestAnimationFrame(sample); else resolve();
                };
                requestAnimationFrame(sample);
              });
              return frames;
            }""", GEOMETRY)
            (output / "geometry.json").write_text(json.dumps({"baseline": baseline, "frames": frames}, indent=2) + "\n", encoding="utf-8")
            last = frames[-1]
            check("animation: card never grows", all(abs(frame["card"]["height"] - baseline["card"]["height"]) <= 1 for frame in frames), [baseline["card"]["height"], min(frame["card"]["height"] for frame in frames), max(frame["card"]["height"] for frame in frames)])
            check("animation: bottom actions stay in place", all(abs(frame["button"]["y"] - baseline["button"]["y"]) <= 1 for frame in frames))
            check("animation: presentation geometry is stable", all(abs(frame["presentation"]["height"] - baseline["presentation"]["height"]) <= 1 for frame in frames))
            check("animation: avatar becomes inaccessible", last["accountInert"] and last["account"]["opacity"] < 0.01)
            check("animation: progress replaces avatar", not last["progressInert"] and last["progress"]["opacity"] > 0.99 and abs(last["progressBox"]["y"] - last["presentation"]["y"]) <= 1)
            check("animation: no horizontal content overflow", all(frame["contentOverflow"]["x"] <= 1 and frame["cardOverflow"] <= 1 for frame in frames), last["contentOverflow"])
            check("animation: no unnecessary vertical scroll", baseline["contentOverflow"]["y"] <= 1 and last["contentOverflow"]["y"] <= 1, [baseline["contentOverflow"], last["contentOverflow"]])
            reduced = page.evaluate("matchMedia('(prefers-reduced-motion: reduce)').matches")
            if not reduced:
                check("animation: visible intermediate floating frames", any(0.02 < frame["progress"]["opacity"] < 0.98 for frame in frames))
                check("animation: translation only, no scale expansion", all("scale" not in frame["progress"]["transform"] and (frame["progress"]["transform"] == "none" or frame["progress"]["transform"].startswith("matrix(1, 0, 0, 1,")) for frame in frames))
            else:
                check("animation: reduced motion immediate", all(frame["progress"]["opacity"] > 0.99 and frame["progress"]["transform"] == "none" for frame in frames))
            expect(page.get_by_role("button", name=CANCEL, exact=True)).to_be_enabled()
            expect(page.get_by_role("button", name="\u9009\u62e9\u7248\u672c", exact=True)).to_be_disabled()
            event(page, {"type": "progress", "downloaded": 32, "total": 64, "fileType": "Assets"})
            event(page, {"type": "speed", "bps": 1048576})
            expect(page.get_by_role("progressbar")).to_have_attribute("aria-valuenow", "50")
            page.get_by_role("button", name=CANCEL, exact=True).click()
            expect(page.locator(".launch-panel__cancel")).to_be_disabled()
            check("animation: cancel issued once", len(calls(page, "launch_cancel")) == 1)
            event(page, {"type": "cancelled"})
            page.wait_for_timeout(300)
            restored = page.evaluate(GEOMETRY)
            check("animation: cancellation restores avatar without expansion", not restored["accountInert"] and restored["progressInert"] and abs(restored["card"]["height"] - baseline["card"]["height"]) <= 1)
            expect(page.locator(".launch-button")).to_be_enabled()

        for width, height, scale, reduced in [(1100, 600, 1, "no-preference"), (1100, 720, 1, "no-preference"), (1100, 1050, 1, "no-preference"), (900, 720, 1.5, "no-preference"), (760, 850, 1, "no-preference"), (1100, 720, 1, "reduce")]:
            run_case(f"home-{width}-{height}-{scale}-{reduced}", {"mode": "home"}, home_geometry, width, height, scale, reduced)

        def resize_card(page, output):
            heights = []
            for height in [600, 720, 1050, 720]:
                page.set_viewport_size({"width": 1100, "height": height})
                page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
                heights.append({"height": height, **page.evaluate(GEOMETRY)})
            (output / "resizing.json").write_text(json.dumps(heights, indent=2), encoding="utf-8")
            check("resize: card grows with window", abs((heights[2]["card"]["height"] - heights[1]["card"]["height"]) - 330) <= 1, heights)
            check("resize: original size restored", abs(heights[1]["card"]["height"] - heights[3]["card"]["height"]) <= 1)
            check("resize: all heights fit content", all(item["contentOverflow"]["y"] <= 1 for item in heights), heights)

        run_case("home-live-resize", {"mode": "home"}, resize_card)
        run_case("home-online-account", {"mode": "home", "accountType": "microsoft"}, home_geometry)
        run_case("home-long-instance-name", {"mode": "home", "displayName": "Workshop-" + "long-name-" * 40}, home_geometry, width=900, height=720)

        def repeated(page, output):
            page.evaluate("() => { window.__launchFixture.launch(); window.__launchFixture.launch(); window.__launchFixture.launch(); }")
            page.wait_for_function("window.__launchFixture.snapshot().state.starting")
            check("hook: same-turn launch only invokes once", len(calls(page, "launch_game")) == 1)
            event(page, {"type": "progress", "downloaded": "30", "total": "100", "fileType": "Libraries"})
            check("hook: string number progress", state(page)["percent"] == 30)
            event(page, {"type": "started"})
            check("hook: started closes preparation", state(page)["running"] and not state(page)["starting"] and state(page)["progress"] is None)
            page.evaluate("window.__launchFixture.launch()")
            check("hook: running launch is blocked", len(calls(page, "launch_game")) == 1)
            event(page, {"type": "progress", "downloaded": 99, "total": 100})
            event(page, {"type": "stage", "message": "Late stage"})
            event(page, {"type": "speed", "bps": 99})
            check("hook: late preparation events are ignored", state(page)["progress"] is None and state(page)["stage"] == "" and state(page)["speedBps"] is None)
            event(page, {"type": "close", "code": 0})
            launch(page)
            check("hook: can launch after exit", len(calls(page, "launch_game")) == 2)

        run_case("hook-repeat-and-late-progress", {"mode": "hook"}, repeated)

        def numeric(page, output):
            launch(page)
            for payload, expected in [({"downloaded": 200, "total": 100}, 100), ({"downloaded": -20, "total": 100}, 0), ({"downloaded": "Infinity", "total": ""}, None), ({"downloaded": "", "total": "NaN"}, None)]:
                event(page, {"kind": "download", **payload})
                check("hook: progress percent is finite and bounded", state(page)["percent"] == expected, state(page))
            event(page, {"type": "speed", "bps": "Infinity"})
            check("hook: nonfinite speed ignored", state(page)["speedBps"] is None)
            event(page, {"kind": "status", "text": "Ready"})
            check("hook: legacy stage aliases", state(page)["stage"] == "Ready")

        run_case("hook-numeric-boundaries", {"mode": "hook"}, numeric)

        def cancel_retry(page, output):
            launch(page)
            page.evaluate("() => { window.__launchFixture.cancel(); window.__launchFixture.cancel(); }")
            page.wait_for_function("window.__launchFixture.snapshot().state.cancelRequested === false")
            check("hook: cancel failure unlocks retry", len(calls(page, "launch_cancel")) == 1 and state(page)["starting"])
            page.evaluate("window.__launchFixture.cancel()")
            page.wait_for_function("window.__launchFixture.snapshot().state.cancelRequested")
            event(page, {"type": "cancelled"})
            check("hook: completed cancellation clears state", not state(page)["running"] and not state(page)["starting"] and not state(page)["cancelRequested"] and state(page)["error"] == "")
            launch(page)
            check("hook: cancellation allows retry", len(calls(page, "launch_game")) == 2)

        run_case("hook-cancel-failure-retry", {"mode": "hook", "responses": {"launch_cancel": [{"error": "Simulated cancel failure"}, {"value": None}]}}, cancel_retry)

        def old_rejection(page, output):
            launch(page)
            event(page, {"type": "cancelled"})
            launch(page, {"versionId": "1.21.4", "gameDir": "C:/launch-acceptance/new-instance"})
            page.evaluate("window.__launchFixture.release('old-launch')")
            page.wait_for_timeout(100)
            check("hook: old invocation rejection cannot end new launch", state(page)["starting"] and state(page)["error"] == "" and state(page)["reportTarget"]["versionId"] == "1.21.4")

        run_case("hook-previous-invocation-rejection", {"mode": "hook", "responses": {"launch_game": [{"id": "old-launch", "hold": True, "error": "Old invocation rejection"}, {"value": None}]}}, old_rejection)

        def previous_session_events(page, output):
            launch(page)
            old_id = calls(page, "launch_game")[-1]["payload"].get("launchId", "old-launch")
            event(page, {"type": "cancelled", "launchId": old_id})
            launch(page, {"versionId": "1.21.4", "gameDir": "C:/launch-acceptance/new-instance"})
            new_id = calls(page, "launch_game")[-1]["payload"].get("launchId", "new-launch")
            for payload in [
                {"type": "close", "code": 1},
                {"type": "error", "message": "Previous session error"},
                {"type": "diagnostic", "summary": "Previous session crash", "message": "Old advice"},
                {"type": "warning", "message": "Previous session warning"},
                {"type": "progress", "downloaded": 10, "total": 20},
                {"type": "started"},
            ]:
                event(page, {**payload, "launchId": old_id})
            current = state(page)
            check("hook: previous session events cannot mutate new launch", current["starting"] and current["error"] == "" and current["warning"] == "" and current["diagnostic"] is None and current["progress"] is None, current)
            check("hook: every launch has a distinct event identity", old_id != new_id and "launchId" in calls(page, "launch_game")[-1]["payload"])
            event(page, {"type": "started", "launchId": new_id})
            check("hook: current session events remain accepted", state(page)["running"] and not state(page)["starting"])

        run_case("hook-previous-session-late-events", {"mode": "hook"}, previous_session_events)

        def missing_listener(page, output):
            page.evaluate("window.__launchFixture.launch()")
            page.wait_for_timeout(150)
            current = state(page)
            check("hook: failed listener never starts an unobservable launch", not current["starting"] and not current["running"] and bool(current["error"]) and not calls(page, "launch_game"), current)

        run_case("hook-listener-failure-fails-closed", {"mode": "hook", "listenFailures": 3}, missing_listener)


        def unmount(page, output):
            launch(page)
            page.evaluate("window.__launchFixture.unmount()")
            page.evaluate("window.__launchFixture.release('pending-launch')")
            page.wait_for_timeout(250)
            check("hook: unmounted listeners are released", snapshot(page)["listeners"].get("launcher-event", 0) == 0, snapshot(page)["listeners"])
            event(page, {"type": "started"})
            check("hook: late unmounted events do not commit", not page.locator('[data-testid="hook-state"]').count())

        run_case("hook-unmount-late-rejection", {"mode": "hook", "responses": {"launch_game": [{"id": "pending-launch", "hold": True, "error": "Late after unmount"}]}}, unmount)

        def delayed_listener(page, output):
            page.evaluate("window.__launchFixture.unmount()")
            page.wait_for_timeout(550)
            check("hook: pending listen resolution cleans up", snapshot(page)["listeners"].get("launcher-event", 0) == 0, snapshot(page))

        run_case("hook-unmount-pending-listen", {"mode": "hook", "listenDelayMs": 450}, delayed_listener)

        def hook_navigation(page, output):
            launch(page)
            launch_id = calls(page, "launch_game")[-1]["payload"]["launchId"]
            target = state(page)["reportTarget"]
            event(page, {"type": "preparing", "launchId": launch_id, **target})
            page.evaluate("window.__launchFixture.navigate('settings')")
            expect(page.get_by_test_id("hook-state")).to_have_count(0)
            check("navigation: controller remains subscribed", snapshot(page)["listeners"].get("launcher-event") == 1)
            event(page, {"type": "progress", "launchId": launch_id, "downloaded": 30, "total": 100})
            page.evaluate("window.__launchFixture.navigate('home')")
            expect(page.get_by_test_id("hook-state")).to_be_visible()
            check("navigation: preparing state and progress survive", state(page)["starting"] and state(page)["percent"] == 30 and state(page)["reportTarget"] == target, state(page))
            event(page, {"type": "error", "launchId": "unrelated-launch", "message": "Wrong failure"})
            check("navigation: foreign event remains rejected", state(page)["starting"] and not state(page)["error"])
            page.evaluate("window.__launchFixture.cancel()")
            check("navigation: cancel keeps original launch identity", calls(page, "launch_cancel")[-1]["payload"]["launchId"] == launch_id)
            event(page, {"type": "error", "launchId": launch_id, "message": "Scoped failure after return"})
            check("navigation: scoped error remains visible", state(page)["error"] == "Scoped failure after return" and not state(page)["starting"] and state(page)["reportTarget"] == target)

        run_case("hook-navigation-preserves-progress-cancel-and-error", {"mode": "hook"}, hook_navigation)

        def navigation_rejection(page, output):
            launch(page)
            target = state(page)["reportTarget"]
            page.evaluate("window.__launchFixture.navigate('settings')")
            expect(page.get_by_test_id("hook-state")).to_have_count(0)
            page.evaluate("window.__launchFixture.release('navigation-launch')")
            page.wait_for_timeout(100)
            page.evaluate("window.__launchFixture.navigate('home')")
            expect(page.get_by_test_id("hook-state")).to_be_visible()
            check("navigation: invoke failure survives absent consumer", state(page)["error"] == "Failure while away" and state(page)["reportTarget"] == target and not state(page)["running"], state(page))

        run_case("hook-navigation-pending-invoke-failure", {"mode": "hook", "responses": {"launch_game": [{"id": "navigation-launch", "hold": True, "error": "Failure while away"}]}}, navigation_rejection)

        def home_navigation(page, output, started):
            page.locator(".launch-button").click()
            payload = calls(page, "launch_game")[-1]["payload"]
            launch_id = payload["launchId"]
            event(page, {"type": "preparing", "launchId": launch_id, "versionId": payload["versionId"], "gameDir": payload["gameDir"]})
            if started:
                event(page, {"type": "started", "launchId": launch_id})
            page.get_by_role("button", name="\u8bbe\u7f6e", exact=True).click()
            expect(page.locator(".home__card")).to_have_count(0)
            check("home navigation: only one controller listener", snapshot(page)["listeners"].get("launcher-event") == 1)
            page.evaluate("window.__launchFixture.select({ root: 'C:/launch-acceptance/different-instance', versionId: '1.21.4', displayName: 'Different instance', loader: 'vanilla', icon: 'grass' })")
            if started:
                event(page, {"type": "close", "launchId": launch_id, "code": 1})
                event(page, {"type": "diagnostic", "launchId": launch_id, "category": "memory", "title": "Memory", "summary": "Failure while away", "message": "Allocate more memory and retry."})
            else:
                event(page, {"type": "error", "launchId": launch_id, "message": "Failure while away"})
            page.get_by_role("button", name="\u9996\u9875", exact=True).click()
            expect(page.get_by_role("dialog")).to_be_visible()
            expect(page.locator(".launch-error__reason p")).to_have_text("Failure while away")
            page.get_by_role("button", name=LOGS, exact=True).click()
            page.get_by_role("button", name=EXPORT, exact=True).click()
            expect(page.get_by_text(SUCCESS, exact=True)).to_be_visible()
            for command in ["launch_log_open", "launch_error_report_export"]:
                report = calls(page, command)[-1]["payload"]
                check("home navigation: original report identity survives selection change", report["dir"] == payload["gameDir"] and report["versionId"] == payload["versionId"], report)
            check("home navigation: report contains failure captured off page", "Failure while away" in calls(page, "launch_error_report_export")[-1]["payload"]["failure"])
            check("home navigation: returning never relaunches", len(calls(page, "launch_game")) == 1)

        for started in [False, True]:
            run_case("home-navigation-" + ("running-close-diagnostic" if started else "preparing-error"), {"mode": "home", "responses": {"launch_error_report_export": [{"value": "C:/launch-acceptance/navigation-report.zip"}]}}, lambda page, output, started=started: home_navigation(page, output, started))

        def busy_export(page, output):
            export = page.get_by_role("button", name=EXPORT, exact=True)
            export.evaluate("button => { button.click(); button.click(); }")
            expect(page.get_by_role("button", name=BUSY, exact=True)).to_be_disabled()
            check("dialog: duplicate export invokes once", len(calls(page, "launch_error_report_export")) == 1)
            check("dialog: all footer controls disabled while saving", page.locator(".ni-modal__foot button:not(:disabled)").count() == 0)
            check("dialog: close icon unavailable while saving", page.get_by_role("dialog").get_by_role("button", name="\u5173\u95ed", exact=True).count() == 0)
            page.keyboard.press("Escape")
            page.locator(".ni-modal__backdrop").click(position={"x": 2, "y": 2})
            expect(page.get_by_role("dialog")).to_be_visible()
            check("dialog: busy dismissal does not call close", snapshot(page)["closeCount"] == 0)
            page.evaluate("window.__launchFixture.release('report')")
            expect(page.get_by_text(SUCCESS, exact=True)).to_be_visible()
            expect(page.get_by_role("button", name=EXPORT, exact=True)).to_be_enabled()
            page.get_by_role("button", name=KNOWN, exact=True).click()
            expect(page.get_by_role("dialog")).not_to_be_visible()

        run_case("dialog-busy-success-nondismissible", {"mode": "dialog", "responses": {"launch_error_report_export": [{"id": "report", "hold": True, "value": "C:/launch-acceptance/report.zip"}]}}, busy_export)

        def export_retry(page, output):
            expect(page.locator(".launch-error__reason p")).to_have_text("OutOfMemoryError")
            text = page.get_by_role("dialog").inner_text()
            check("dialog: short reason replaces raw stack", "complete stack stays in report" not in text)
            check("dialog: complete ZIP and no screenshot guidance", "ZIP" in text and "\u4e0d\u8981\u53ea\u622a\u56fe" in text and "\u4e00\u4efd\u65e5\u5fd7" in text)
            page.get_by_role("button", name=EXPORT, exact=True).click()
            expect(page.get_by_role("button", name=EXPORT, exact=True)).to_be_enabled()
            check("dialog: cancelling save is not success", not page.get_by_text(SUCCESS, exact=True).count() and not page.get_by_text(FAILED, exact=True).count())
            page.get_by_role("button", name=EXPORT, exact=True).click()
            expect(page.get_by_text(FAILED, exact=True)).to_be_visible()
            page.get_by_role("button", name=EXPORT, exact=True).click()
            expect(page.get_by_text(SUCCESS, exact=True)).to_be_visible()
            check("dialog: success clears previous failure", not page.get_by_text(FAILED, exact=True).count())
            check("dialog: cancel/failure/retry calls all preserved", len(calls(page, "launch_error_report_export")) == 3)
            page.get_by_role("button", name=LOGS, exact=True).click()
            check("dialog: open logs remains reachable", len(calls(page, "launch_log_open")) == 1)

        run_case("dialog-save-cancel-failure-retry", {"mode": "dialog", "responses": {"launch_error_report_export": [{"value": None}, {"error": "Simulated disk locked; retry is safe"}, {"value": "C:/launch-acceptance/report.zip"}]}}, export_retry)

        def diagnostic(page, output):
            expect(page.get_by_role("dialog")).to_have_accessible_name("\u6e38\u620f\u610f\u5916\u9000\u51fa\u4e86")
            expect(page.locator(".launch-error__reason p")).to_have_text("Insufficient Java heap")
            expect(page.locator(".launch-error__message")).to_have_text("Increase allocated memory and try again.")
            values = page.evaluate(r"""() => [
              window.__launchFixture.errorInfo('', null),
              window.__launchFixture.errorInfo('\n\tFirst reason\u0007\nLong stack', null),
              window.__launchFixture.errorInfo('Timeout while downloading\nFull stack', null),
              window.__launchFixture.errorInfo('x'.repeat(1000), null),
              window.__launchFixture.errorInfo('\ud83d\ude00'.repeat(300), null)
            ]""")
            check("dialog: empty failure has no dialog info", values[0] is None)
            check("dialog: first nonempty line and controls sanitized", values[1]["reason"] == "First reason ")
            check("dialog: network advice is actionable", "\u7f51\u7edc" in values[2]["message"])
            check("dialog: long reasons are bounded", len(values[3]["reason"]) <= 242 and values[3]["reason"].endswith("..."))
            check("dialog: unicode truncation keeps complete characters", len(values[4]["reason"]) <= 242 and "\ufffd" not in values[4]["reason"])

        run_case("dialog-diagnostic-and-reason-boundaries", {"mode": "dialog", "diagnostic": {"category": "memory", "title": "Memory", "summary": "Insufficient Java heap\nFull evidence", "message": "Increase allocated memory and try again."}}, diagnostic)

        def failed_target(page, output):
            page.locator(".launch-button").click()
            event(page, {"type": "started"})
            event(page, {"type": "close", "code": 1})
            event(page, {"type": "diagnostic", "category": "memory", "title": "Memory", "summary": "OutOfMemoryError", "message": "Allocate more memory and retry."})
            expect(page.get_by_role("dialog")).to_be_visible()
            page.evaluate("window.__launchFixture.select({ root: 'C:/launch-acceptance/different-instance', versionId: '1.21.4', displayName: 'Different instance', loader: 'vanilla', icon: 'grass' })")
            page.get_by_role("button", name=LOGS, exact=True).click()
            page.get_by_role("button", name=EXPORT, exact=True).click()
            expect(page.get_by_text(SUCCESS, exact=True)).to_be_visible()
            for command in ["launch_log_open", "launch_error_report_export"]:
                payload = calls(page, command)[-1]["payload"]
                check("home: failed target is independent of current selection", payload["dir"] == "C:/launch-acceptance/early-instance" and payload["versionId"] == "1.21.1", payload)
            check("home: report includes diagnostic cause", "OutOfMemoryError" in calls(page, "launch_error_report_export")[-1]["payload"]["failure"])
            page.get_by_role("button", name=KNOWN, exact=True).click()
            expect(page.locator(".launch-button")).to_be_enabled()
            page.locator(".launch-button").click()
            check("home: next launch targets new selection", calls(page, "launch_game")[-1]["payload"]["gameDir"] == "C:/launch-acceptance/different-instance")

        run_case("home-failed-instance-report-snapshot", {"mode": "home", "responses": {"launch_error_report_export": [{"value": "C:/launch-acceptance/report.zip"}]}}, failed_target)
        browser_version = browser.version
        browser.close()

    summary = {"scope": "real headless browser; current production Home, launch hook and crash dialog; simulated IPC", "nativeAcceptance": False, "browserVersion": browser_version, "cases": len(results), "checks": len(checks), "passed": all(result["passed"] for result in results), "failures": [result["name"] for result in results if not result["passed"]], "results": [{"name": result["name"], "passed": result["passed"], "error": result["error"]} for result in results]}
    (args.evidence_dir / "browser-results.json").write_text(json.dumps(summary, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True), flush=True)
    raise SystemExit(0 if summary["passed"] else 1)


if __name__ == "__main__":
    main()
