"""Launch transitions, cancellation ownership and independent session regressions."""
import json
from playwright.sync_api import expect

START = "\u542f\u52a8\u6e38\u620f"
CANCEL = "\u53d6\u6d88\u542f\u52a8"
KNOWN = "\u77e5\u9053\u4e86"
LOGS = "\u6253\u5f00\u65e5\u5fd7\u76ee\u5f55"
CHECK = "\u6821\u9a8c:"


def run_transition_checks(run_case, check, event, calls, state):
    def settled(page):
        page.evaluate("""() => Promise.all(document.getAnimations()
            .filter(a => a.effect?.getTiming().iterations !== Infinity)
            .map(a => a.finished.catch(() => {})))""")

    def progress_phases(page, output):
        button = page.locator(".launch-button")
        button.focus()
        page.keyboard.press("Enter")
        expect(button).to_have_accessible_name(CANCEL)
        expect(button).to_be_focused()
        expect(page.locator(".launch-panel button")).to_have_count(0)
        expect(page.get_by_role("progressbar")).to_have_count(0)
        check("phases: preparation has no fabricated progress", page.locator('.progress__percent-reveal[data-open="true"]').count() == 0)
        settled(page)
        baseline = button.bounding_box()
        event(page, {"type": "progress", "downloaded": 12, "total": 40, "fileType": CHECK + "libraries"})
        expect(page.get_by_role("progressbar")).to_have_attribute("aria-valuenow", "30")
        expect(page.locator(".progress__count")).to_have_text("12 / 40")
        expect(page.locator(".progress__speed-reveal")).to_have_attribute("data-open", "false")
        settled(page)
        page.screenshot(path=str(output / "checking.png"))
        event(page, {"type": "progress", "downloaded": 2, "total": 10, "fileType": "assets"})
        event(page, {"type": "speed", "bps": 1048576})
        expect(page.locator(".progress__speed")).to_have_text("1.0 MB/s")
        event(page, {"type": "estimated", "seconds": 12.5})
        expect(page.get_by_role("progressbar")).to_have_attribute("aria-valuenow", "20")
        expect(page.locator(".progress__speed-reveal")).to_have_attribute("data-open", "true")
        settled(page)
        page.screenshot(path=str(output / "downloading.png"))
        bounds = page.locator(".launch-panel").bounding_box()
        check("phases: expanded progress does not overlap primary action", bounds["y"] + bounds["height"] < button.bounding_box()["y"], bounds)
        event(page, {"type": "speed", "bps": 0})
        expect(page.locator(".progress__speed")).to_have_text("0 B/s")
        event(page, {"type": "progress", "downloaded": 3, "total": 0, "fileType": "assets"})
        expect(page.get_by_role("progressbar")).not_to_have_attribute("aria-valuenow", "0")
        check("phases: unknown totals omit numeric ARIA value", page.get_by_role("progressbar").get_attribute("aria-valuenow") is None)
        expect(page.locator(".progress__percent-reveal")).to_have_attribute("aria-hidden", "true")
        expect(page.locator(".progress__count")).to_have_text("3")
        settled(page)
        event(page, {"type": "progress", "downloaded": 5, "total": 10, "fileType": "assets"})
        settled(page)
        frames = page.evaluate("""async () => {
            const launchId = window.__launchFixture.snapshot().calls.filter(c => c.command === 'launch_game').at(-1).payload.launchId;
            await window.__launchFixture.event({type: 'stage', message: 'Extracting files', launchId});
            const frames = [], start = performance.now();
            await new Promise(resolve => {
                const sample = () => {
                    const row = document.querySelector('.progress__percent-reveal');
                    frames.push({opacity: Number(getComputedStyle(row).opacity), height: row.getBoundingClientRect().height,
                        text: row.textContent, hidden: row.getAttribute('aria-hidden')});
                    if (performance.now() - start < 450) requestAnimationFrame(sample); else resolve();
                };
                requestAnimationFrame(sample);
            });
            return frames;
        }""")
        (output / "progress-exit.json").write_text(json.dumps(frames, indent=2), encoding="utf-8")
        reduced = page.evaluate("matchMedia('(prefers-reduced-motion: reduce)').matches")
        if not reduced:
            check("phases: numeric row fades out instead of unmounting", any(0.01 < frame["opacity"] < 0.99 and "50%" in frame["text"] for frame in frames), frames)
            check("phases: numeric row collapses continuously", any(0 < frame["height"] < frames[0]["height"] for frame in frames[1:]), frames)
        else:
            check("phases: reduced motion hides values immediately", all(frame["opacity"] == 0 for frame in frames), frames)
        expect(page.get_by_role("progressbar")).to_have_count(0)
        expect(page.locator(".progress__speed-reveal")).to_have_attribute("data-open", "false")
        check("phases: bottom action remains stable", abs(button.bounding_box()["y"] - baseline["y"]) <= 1)
        event(page, {"type": "speed", "bps": 2048})
        expect(page.get_by_role("progressbar")).to_be_visible()
        expect(page.locator(".progress__percent-reveal")).to_have_attribute("data-open", "false")
        expect(page.locator(".progress__count-reveal")).to_have_attribute("data-open", "false")
        event(page, {"type": "progress", "downloaded": 10, "total": 10, "fileType": CHECK + "assets"})
        expect(page.locator(".progress__speed-reveal")).to_have_attribute("data-open", "false")
        event(page, {"type": "stage", "message": "Ready to launch"})
        expect(page.get_by_role("progressbar")).to_have_count(0)
        event(page, {"type": "started"})
        expect(button).to_have_accessible_name(START)
        expect(button).to_be_enabled()
        if not reduced:
            check("phases: outgoing panel retains its last stage", "Ready to launch" in page.locator(".launch-panel").text_content())
        settled(page)
        expect(page.locator(".home__launch-progress")).to_have_attribute("aria-hidden", "true")
        page.screenshot(path=str(output / "restored.png"))
        check("phases: completed launch never displays a running label", "\u6e38\u620f\u8fd0\u884c\u4e2d" not in page.locator(".home").inner_text())

    for reduced in ["no-preference", "reduce"]:
        run_case("home-progress-phases-" + reduced, {"mode": "home"}, progress_phases, reduced=reduced)

    run_case("home-progress-small-window", {"mode": "home"}, progress_phases, width=900, height=600)

    def cancel_race(page, output):
        button = page.locator(".launch-button")
        button.click()
        button.evaluate("button => { button.click(); button.click(); }")
        expect(button).to_be_disabled()
        original = calls(page, "launch_game")[0]["payload"]
        event(page, {"type": "preparing", **original})
        expect(button).to_be_disabled()
        check("cancel: delayed preparing cannot unlock cancellation", len(calls(page, "launch_cancel")) == 1)
        event(page, {"type": "progress", "downloaded": 9, "total": 10})
        expect(page.get_by_role("progressbar")).to_have_count(0)
        page.evaluate("window.__launchFixture.release('cancel')")
        expect(button).to_be_enabled()
        expect(button).to_have_accessible_name(CANCEL)
        button.focus()
        page.keyboard.press("Space")
        expect(button).to_be_disabled()
        check("cancel: keyboard retry belongs to same launch", len(calls(page, "launch_cancel")) == 2 and calls(page, "launch_cancel")[-1]["payload"]["launchId"] == original["launchId"])
        event(page, {"type": "cancelled"})
        expect(button).to_have_accessible_name(START)
        expect(button).to_be_enabled()
        button.click()
        check("cancel: next attempt uses a fresh identity", calls(page, "launch_game")[-1]["payload"]["launchId"] != original["launchId"])
        event(page, {"type": "started"})
        expect(button).to_be_enabled()

    run_case("home-cancel-race-and-keyboard", {"mode": "home", "responses": {"launch_cancel": [{"id": "cancel", "hold": True, "error": "Retry cancellation"}, {"value": None}]}}, cancel_race)

    def multi_instance(page, output):
        button = page.locator(".launch-button")
        button.click()
        first = calls(page, "launch_game")[-1]["payload"]
        event(page, {"type": "started", "launchId": first["launchId"]})
        expect(button).to_be_enabled()
        expect(button).to_have_accessible_name(START)
        page.evaluate("window.__launchFixture.select({ root: 'C:/launch-acceptance/early-instance', versionId: '1.21.4', displayName: 'Second instance', loader: 'vanilla', icon: 'grass' })")
        expect(button).to_be_enabled()
        button.click()
        second = calls(page, "launch_game")[-1]["payload"]
        check("multi: first process does not block second preparation", len(calls(page, "launch_game")) == 2 and first["launchId"] != second["launchId"])
        event(page, {"type": "progress", "downloaded": 4, "total": 10})
        event(page, {"type": "close", "code": 1, "launchId": first["launchId"]})
        event(page, {"type": "diagnostic", "launchId": first["launchId"], "title": "First failure", "summary": "First instance failure", "message": "Check the first instance"})
        expect(button).to_have_accessible_name(CANCEL)
        expect(page.get_by_role("progressbar")).to_have_attribute("aria-valuenow", "40")
        expect(page.get_by_role("dialog")).to_have_count(0)
        page.evaluate("window.__launchFixture.select({ root: 'C:/launch-acceptance/other', versionId: '1.20.1', displayName: 'Other selection', loader: 'vanilla', icon: 'grass' })")
        expect(page.locator(".launch-panel__name")).to_have_text("Second instance")
        button.click()
        check("multi: cancel targets only active preparation", calls(page, "launch_cancel")[-1]["payload"]["launchId"] == second["launchId"])
        event(page, {"type": "cancelled", "launchId": second["launchId"]})
        expect(page.get_by_role("dialog")).to_be_visible()
        expect(page.locator(".launch-error__reason p")).to_have_text("First instance failure")
        page.get_by_role("button", name=LOGS, exact=True).click()
        report = calls(page, "launch_log_open")[-1]["payload"]
        check("multi: deferred failure retains original instance", report["versionId"] == first["versionId"] and report["dir"] == first["gameDir"])
        page.get_by_role("button", name=KNOWN, exact=True).click()
        expect(button).to_be_enabled()
        button.click()
        third = calls(page, "launch_game")[-1]["payload"]
        event(page, {"type": "progress", "downloaded": 9, "total": 10, "launchId": second["launchId"]})
        expect(page.get_by_role("progressbar")).to_have_count(0)
        check("multi: selection after cancellation is used for the next launch", third["versionId"] == "1.20.1")
        event(page, {"type": "started"})
        expect(button).to_be_enabled()

    run_case("home-multi-instance-ownership", {"mode": "home"}, multi_instance)

    def session_isolation(page, output):
        page.evaluate("window.__launchFixture.launch()")
        page.wait_for_function("window.__launchFixture.snapshot().state.starting")
        first = calls(page, "launch_game")[-1]["payload"]
        event(page, {"type": "started"})
        page.evaluate("window.__launchFixture.launch({versionId: 'second'})")
        page.wait_for_function("window.__launchFixture.snapshot().state.starting")
        second = calls(page, "launch_game")[-1]["payload"]
        event(page, {"type": "progress", "downloaded": 3, "total": 10})
        event(page, {"type": "started", "launchId": first["launchId"]})
        event(page, {"type": "progress", "launchId": first["launchId"], "downloaded": 10, "total": 10})
        event(page, {"type": "close", "launchId": first["launchId"], "code": 0})
        check("multi: old completion leaves new preparation intact", state(page)["starting"] and state(page)["percent"] == 30)
        event(page, {"type": "started", "launchId": second["launchId"]})
        check("multi: aggregate running includes remaining game", state(page)["running"] and not state(page)["starting"])
        event(page, {"type": "close", "launchId": second["launchId"], "code": 0})
        check("multi: last exit clears aggregate running", not state(page)["running"])

    run_case("hook-independent-session-exits", {"mode": "hook"}, session_isolation)

    def queued_reports(page, output):
        button = page.locator(".launch-button")
        sessions = []
        for version in ["first", "second", "third"]:
            page.evaluate("versionId => window.__launchFixture.select({ root: 'C:/launch-acceptance/isolated', versionId, displayName: versionId, loader: 'vanilla', icon: 'grass' })", version)
            expect(button).to_be_enabled()
            button.click()
            sessions.append(calls(page, "launch_game")[-1]["payload"])
            if version != "third":
                event(page, {"type": "started"})
        for index in [0, 1]:
            launch_id = sessions[index]["launchId"]
            event(page, {"type": "close", "launchId": launch_id, "code": 1})
            event(page, {"type": "diagnostic", "launchId": launch_id, "summary": "Failure " + str(index)})
        expect(page.get_by_role("dialog")).to_have_count(0)
        button.click()
        event(page, {"type": "cancelled"})
        for index in [0, 1]:
            expect(page.locator(".launch-error__reason p")).to_have_text("Failure " + str(index))
            expect(page.locator(".launch-error")).not_to_contain_text("\u5b8c\u6574\u62a5\u544a\u5df2\u5bfc\u51fa")
            page.get_by_role("button", name="\u5bfc\u51fa\u9519\u8bef\u62a5\u544a ZIP", exact=True).click()
            expect(page.locator(".launch-error")).to_contain_text("\u5b8c\u6574\u62a5\u544a\u5df2\u5bfc\u51fa")
            report = calls(page, "launch_error_report_export")[-1]["payload"]
            check("multi: each queued export keeps its own target", report["versionId"] == sessions[index]["versionId"] and "Failure " + str(index) in report["failure"])
            page.get_by_role("button", name=KNOWN, exact=True).click()
        expect(page.get_by_role("dialog")).to_have_count(0)
        expect(button).to_be_enabled()

    run_case("home-independent-report-queue", {"mode": "home", "responses": {"launch_error_report_export": [{"value": "C:/reports/first.zip"}, {"value": "C:/reports/second.zip"}]}}, queued_reports)
