import argparse
import hashlib
import json
import sys
import time
import traceback
from datetime import datetime, timezone
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

EULA = "\u5f00\u59cb\u4e4b\u524d\uff0c\u8bf7\u786e\u8ba4 Minecraft EULA"
ACCEPT = "\u540c\u610f\u5e76\u7b7e\u7f72"
REJECT = "\u62d2\u7edd"
ACK = "\u6211\u5df2\u77e5\u6653"
RETRY = "\u91cd\u8bd5"
EXIT = "\u65e0\u6cd5\u7ee7\u7eed\u63d0\u4f9b\u670d\u52a1"
REVISION = "minecraft-eula-v1"
VERSION = "1.0.3"
ACCEPTED = {"eulaVersion": REVISION, "eulaAcceptedAt": 1700000000}
COMPLETE = {**ACCEPTED, "changelogSeenVersion": VERSION}


def snapshot(page):
    return page.evaluate("window.__startupFixture.snapshot()")


def calls(state, command):
    return [entry for entry in state["events"] if entry.get("command") == command]


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def is_eula(page):
    expect(page.get_by_role("dialog", name=EULA)).to_be_visible()


def is_changelog(page):
    expect(page.get_by_role("dialog", name="Nickel 1.0.3 \u66f4\u65b0\u8bf4\u660e")).to_be_visible()


def sign(page):
    page.get_by_role("checkbox").click()
    page.get_by_role("button", name=ACCEPT, exact=True).click()
    is_changelog(page)


def wait_ready(page):
    page.wait_for_function("window.__startupFixture.snapshot().shellInert === false")
    expect(page.locator(".startup-modal")).to_have_count(0)


def non_dismiss(page):
    page.keyboard.press("Escape")
    page.locator(".ni-modal__backdrop").click(position={"x": 4, "y": 4})
    expect(page.locator(".startup-modal")).to_have_count(1)
    expect(page.locator(".startup-modal .ni-modal__head button")).to_have_count(0)
    for _ in range(8):
        page.keyboard.press("Tab")
        check(page.evaluate("document.querySelector('.startup-modal').contains(document.activeElement)"), "Tab escaped the gate")
    for _ in range(8):
        page.keyboard.press("Shift+Tab")
        check(page.evaluate("document.querySelector('.startup-modal').contains(document.activeElement)"), "Shift+Tab escaped the gate")
    check(snapshot(page)["shellInert"], "Dismiss attempts unlocked shell")


def main():
    parser = argparse.ArgumentParser(description="Headless production-component acceptance with simulated IPC; never native acceptance.")
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    parser.add_argument("--channel", choices=["chromium", "msedge"], default="chromium")
    parser.add_argument("--executable-path", type=Path, help="Optional installed Chromium executable for this Python Playwright version")
    parser.add_argument("--only", help="Comma-separated case ids")
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    results = []
    cases = []
    selected = set(args.only.split(",")) if args.only else None

    def case(name, setup=None):
        def register(function):
            if selected is None or name in selected:
                cases.append((name, setup or {}, function))
            return function
        return register

    @case("eula-open-failure-without-url", {"failures": {"plugin:opener|open_url": 1}})
    def eula_open_failure(page, output):
        is_eula(page)
        link = page.get_by_role("button", name="\u9605\u8bfb Minecraft EULA \u5b8c\u6574\u534f\u8bae", exact=True)
        link.click()
        expect(page.get_by_role("alert")).to_contain_text("\u65e0\u6cd5\u6253\u5f00\u6d4f\u89c8\u5668")
        check("minecraft.net" not in page.locator(".startup-modal").inner_text(), "Failed opener displayed the removed URL")
        check(snapshot(page)["record"] == {} and snapshot(page)["shellInert"], "Opening terms changed consent or unlocked startup")
        page.screenshot(path=str(output / "failed-opener.png"))
        link.click()
        expect(page.get_by_role("alert")).to_have_count(0)
        opened = calls(snapshot(page), "plugin:opener|open_url")
        check(len(opened) == 2 and [entry["outcome"] for entry in opened] == ["rejected", "resolved"], "The terms button did not recover on retry")
        expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_disabled()


    @case("first-run-order-checkbox-and-shell", {"portablePending": True, "autoLaunch": True})
    def first_run(page, output):
        is_eula(page)
        expect(page.locator(".startup-eula__url")).to_have_count(0)
        check("minecraft.net" not in page.locator(".startup-modal").inner_text(), "Redundant EULA URL remains visible")
        expect(page.get_by_role("checkbox")).to_have_attribute("aria-checked", "false")
        expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_disabled()
        expect(page.locator(".startup-modal input[type='text']")).to_have_count(0)
        page.get_by_role("button", name=ACCEPT, exact=True).evaluate("button => button.click()")
        state = snapshot(page)
        check(not calls(state, "onboarding_accept_eula"), "Unchecked action reached IPC")
        check(state["autoLaunchCount"] == 0 and not calls(state, "portable_bootstrap_status"), "Startup work preceded EULA")
        non_dismiss(page)
        page.get_by_role("checkbox").press("Space")
        expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_enabled()
        page.get_by_role("checkbox").press("Space")
        expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_disabled()
        page.get_by_role("button", name="\u9605\u8bfb Minecraft EULA \u5b8c\u6574\u534f\u8bae", exact=True).click()
        before = int(time.time())
        sign(page)
        state = snapshot(page)
        check(set(state["record"]) == {"eulaVersion", "eulaAcceptedAt"}, "Record contains non-consent data or premature changelog acknowledgment")
        check(state["record"]["eulaVersion"] == REVISION, "Wrong internal revision")
        check(before <= state["record"]["eulaAcceptedAt"] <= int(time.time()), "Acceptance is not a Unix seconds timestamp")
        check(calls(state, "onboarding_accept_eula")[0]["payload"] == {"checked": True, "eulaVersion": REVISION}, "Unexpected consent IPC fields")
        check(not calls(state, "onboarding_finish") and not calls(state, "portable_bootstrap_status"), "Startup work preceded changelog acknowledgment")
        expect(page.locator(".startup-modal .ni-modal__desc")).to_have_count(0)
        check("让每一次" not in page.locator(".startup-modal").inner_text(), "Redundant changelog subtitle remains visible")
        changelog = json.loads((Path(__file__).resolve().parents[2] / "CHANGELOG.json").read_text(encoding="utf-8"))
        for section in changelog["sections"]:
            expect(page.locator(".startup-changelog")).to_contain_text(section["title"])
            for item in section["items"]:
                expect(page.locator(".startup-changelog")).to_contain_text(item)
        non_dismiss(page)
        page.screenshot(path=str(output / "changelog.png"))
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)
        page.wait_for_function("window.__startupFixture.snapshot().events.some(e => e.command === 'portable_bootstrap_start')")
        page.get_by_role("dialog", name="\u6b63\u5728\u51c6\u5907\u4fbf\u643a\u6574\u5408\u5305").get_by_role("button", name="\u540e\u53f0\u7ee7\u7eed", exact=True).click()
        page.get_by_test_id("shell-action").click()
        state = snapshot(page)
        check(state["readyCount"] == 1 and state["shellClicks"] == 1, "Normal shell did not enable exactly once")
        check(state["autoLaunchCount"] == 1, "Simulated automatic startup did not run exactly once")
        ordered = [entry.get("command") for entry in state["events"] if entry.get("command") in {"onboarding_accept_eula", "onboarding_ack_changelog", "onboarding_finish", "portable_bootstrap_status", "portable_bootstrap_start"}]
        check(ordered == ["onboarding_accept_eula", "onboarding_ack_changelog", "onboarding_finish", "portable_bootstrap_status", "portable_bootstrap_start"], "Incorrect startup command order: " + repr(ordered))
        page.reload(wait_until="networkidle")
        wait_ready(page)
        restarted = snapshot(page)
        check(not calls(restarted, "onboarding_accept_eula") and not calls(restarted, "onboarding_ack_changelog"), "Same release re-prompted after restart")
        check(restarted["record"] == state["record"], "Restart changed accepted record")

    @case("concurrent-accept-and-reject", {"delays": {"onboarding_accept_eula": 450}})
    def concurrent_accept(page, output):
        is_eula(page)
        page.get_by_role("checkbox").click()
        page.evaluate("""() => {
            const buttons = [...document.querySelectorAll('.ni-modal__foot button')];
            const accept = buttons.find(b => b.textContent.includes('\u540c\u610f\u5e76\u7b7e\u7f72'));
            const reject = buttons.find(b => b.textContent.includes('\u62d2\u7edd'));
            for (let i = 0; i < 20; i++) accept.dispatchEvent(new MouseEvent('click', {bubbles: true}));
            reject.dispatchEvent(new MouseEvent('click', {bubbles: true}));
        }""")
        expect(page.get_by_role("checkbox")).to_be_disabled()
        expect(page.get_by_role("button", name=REJECT, exact=True)).to_be_disabled()
        is_changelog(page)
        state = snapshot(page)
        check(len(calls(state, "onboarding_accept_eula")) == 1, "Concurrent clicks produced duplicate consent writes")
        check(not calls(state, "onboarding_reject"), "Reject raced an in-flight acceptance")

    @case("concurrent-changelog-ack", {"record": ACCEPTED, "delays": {"onboarding_ack_changelog": 350, "onboarding_finish": 350}})
    def concurrent_ack(page, output):
        is_changelog(page)
        page.get_by_role("button", name=ACK, exact=True).evaluate("button => { for (let i = 0; i < 20; i++) button.dispatchEvent(new MouseEvent('click', {bubbles: true})); }")
        wait_ready(page)
        state = snapshot(page)
        check(len(calls(state, "onboarding_ack_changelog")) == 1 and len(calls(state, "onboarding_finish")) == 1, "Concurrent acknowledgments duplicated IPC")
        check(state["readyCount"] == 1, "onReady was dispatched more than once")

    @case("accept-save-failure-retry", {"failures": {"onboarding_accept_eula": 1}})
    def accept_failure(page, output):
        is_eula(page)
        page.get_by_role("checkbox").click()
        page.get_by_role("button", name=ACCEPT, exact=True).click()
        expect(page.get_by_role("alert")).to_contain_text("Simulated local persistence failure")
        is_eula(page)
        expect(page.get_by_role("checkbox")).to_have_attribute("aria-checked", "true")
        check(snapshot(page)["record"] == {}, "Failed save recorded acceptance")
        page.screenshot(path=str(output / "failed-save.png"))
        page.get_by_role("button", name=ACCEPT, exact=True).click()
        is_changelog(page)
        check(len(calls(snapshot(page), "onboarding_accept_eula")) == 2, "Retry did not issue exactly one replacement save")

    @case("changelog-save-failure-retry", {"record": ACCEPTED, "failures": {"onboarding_ack_changelog": 1}})
    def ack_failure(page, output):
        is_changelog(page)
        page.get_by_role("button", name=ACK, exact=True).click()
        expect(page.get_by_role("alert")).to_be_visible()
        is_changelog(page)
        check(snapshot(page)["record"] == ACCEPTED, "Failed acknowledgment changed consent record")
        check(not calls(snapshot(page), "onboarding_finish"), "Failed acknowledgment unlocked startup")
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)
        check(snapshot(page)["record"]["eulaAcceptedAt"] == ACCEPTED["eulaAcceptedAt"], "Acknowledgment overwrote acceptance time")

    @case("finish-failure-retry", {"record": COMPLETE, "failures": {"onboarding_finish": 1}})
    def finish_failure(page, output):
        expect(page.get_by_role("alert")).to_be_visible()
        check(snapshot(page)["shellInert"] and snapshot(page)["readyCount"] == 0, "Failed finish enabled shell")
        page.get_by_role("button", name=RETRY, exact=True).click()
        wait_ready(page)
        check(len(calls(snapshot(page), "onboarding_finish")) == 2, "Finish retry did not recover")

    @case("read-failure-retry", {"responses": {"onboarding_get": [{"error": "discarded StrictMode read"}, {"error": "Fixture record read denied"}]}})
    def read_failure(page, output):
        expect(page.get_by_role("alert")).to_contain_text("Fixture record read denied")
        check(snapshot(page)["shellInert"], "Read failure unlocked shell")
        page.get_by_role("button", name=RETRY, exact=True).click()
        is_eula(page)

    @case("accepted-restart-no-prompts", {"record": COMPLETE})
    def returning(page, output):
        wait_ready(page)
        state = snapshot(page)
        check(state["record"] == COMPLETE and state["readyCount"] == 1, "Existing acceptance changed on startup")
        check(not calls(state, "onboarding_accept_eula") and not calls(state, "onboarding_ack_changelog"), "Completed record prompted again")
        page.keyboard.press("Escape")
        page.get_by_test_id("shell-action").click()
        check(snapshot(page)["shellClicks"] == 1, "Modal listeners persisted after completion")

    @case("accepted-before-ack-restart", {"record": ACCEPTED})
    def accepted_pending(page, output):
        is_changelog(page)
        page.reload(wait_until="networkidle")
        is_changelog(page)
        check(not calls(snapshot(page), "onboarding_accept_eula"), "Accepted user was asked EULA twice")
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)

    @case("version-upgrade-only-changelog", {"record": {**ACCEPTED, "changelogSeenVersion": "1.0.1"}})
    def upgrade(page, output):
        is_changelog(page)
        non_dismiss(page)
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)
        state = snapshot(page)
        check(state["record"] == COMPLETE and not calls(state, "onboarding_accept_eula"), "Version upgrade reset consent")
        page.reload(wait_until="networkidle")
        wait_ready(page)

    @case("old-user-no-consent-record", {"record": {"changelogSeenVersion": "1.0.1"}})
    def old_user(page, output):
        is_eula(page)
        sign(page)
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)

    @case("outdated-revision-reasks-eula", {"record": {"eulaVersion": "obsolete-terms", "eulaAcceptedAt": 1700000000, "changelogSeenVersion": "1.0.1"}})
    def old_revision(page, output):
        is_eula(page)
        expect(page.get_by_role("checkbox")).to_have_attribute("aria-checked", "false")
        sign(page)
        check(snapshot(page)["record"]["eulaVersion"] == REVISION, "Obsolete revision was not replaced")

    @case("refusal-real-clock-hover-and-restart")
    def refusal(page, output):
        is_eula(page)
        page.evaluate("""() => {
            window.__refusalSamples = [];
            const record = () => {
                const button = document.querySelector('.startup-exit-button');
                if (button && window.__refusalSamples.at(-1)?.text !== button.textContent)
                    window.__refusalSamples.push({text: button.textContent, at: performance.now()});
            };
            window.__refusalObserver = new MutationObserver(record);
            window.__refusalObserver.observe(document.body, {subtree: true, childList: true, characterData: true});
        }""")
        page.get_by_role("button", name=REJECT, exact=True).click()
        expect(page.get_by_role("dialog", name=EXIT)).to_be_visible()
        button = page.locator(".startup-exit-button")
        expect(button).to_have_text("\u6b63\u5728\u9000\u51fa...5")
        expect(button).to_be_disabled()
        expect(page.get_by_role("dialog").locator("button")).to_have_count(1)
        check(not calls(snapshot(page), "onboarding_accept_eula"), "Refusal persisted acceptance")
        page.mouse.move(1, 1)
        page.wait_for_timeout(350)
        style_script = "button => Object.fromEntries([...getComputedStyle(button)].map(key => [key, getComputedStyle(button).getPropertyValue(key)]))"
        style_before = button.evaluate(style_script)
        image_before = button.screenshot(path=str(output / "hover-before.png"))
        button.hover()
        style_hover = button.evaluate(style_script)
        image_hover = button.screenshot(path=str(output / "hover-during.png"))
        differences = {key: [value, style_hover.get(key)] for key, value in style_before.items() if style_hover.get(key) != value}
        check(not differences, "Refusal hover changed computed CSS: " + repr(differences))
        check(image_before == image_hover, "Refusal button screenshot changed on hover")
        page.keyboard.press("Escape")
        page.locator(".ni-modal__backdrop").click(position={"x": 3, "y": 3})
        page.keyboard.press("Tab")
        check(page.evaluate("document.activeElement === document.querySelector('.startup-modal')"), "Disabled-only refusal dialog lost focus trap")
        samples = page.evaluate("window.__refusalSamples")
        start = samples[0]["at"]
        elapsed = page.evaluate("performance.now()") - start
        page.wait_for_timeout(max(0, 4400 - elapsed))
        state_early = snapshot(page)
        check(state_early["exitCount"] == 0, "Simulated backend exit occurred before five real seconds")
        check(not calls(state_early, "window_close"), "Frontend fallback closed before five real seconds")
        page.wait_for_function("window.__startupFixture.snapshot().exitCount === 1", timeout=3000)
        expect(button).to_have_text("\u6b63\u5728\u9000\u51fa...0", timeout=2000)
        state = snapshot(page)
        samples = page.evaluate("window.__refusalSamples")
        page.evaluate("window.__refusalObserver.disconnect()")
        check([sample["text"] for sample in samples] == ["\u6b63\u5728\u9000\u51fa..." + str(value) for value in range(5, -1, -1)], "Real countdown skipped or repeated values: " + repr(samples))
        exit_event = next(entry for entry in state["events"] if entry["kind"] == "simulated-backend-exit")
        scheduled = next(entry for entry in state["events"] if entry["kind"] == "simulated-backend-exit-scheduled")
        check(5000 <= exit_event["at"] - scheduled["at"] <= 6000, "Backend simulation used an accelerated or drifting clock")
        check(state["record"] == {} and state["readyCount"] == 0 and state["shellInert"], "Refusal bypassed onboarding")
        check(len(calls(state, "onboarding_reject")) == 1 and len(calls(state, "window_close")) == 1, "Refusal was not idempotent or fallback did not run")
        (output / "real-clock-and-hover.json").write_text(json.dumps({"nativeAcceptance": False, "clock": "unmodified performance.now and real browser timers", "samples": samples, "exitDelayMs": exit_event["at"] - scheduled["at"], "computedStyleBefore": style_before, "computedStyleHover": style_hover, "styleDifferences": differences, "pngSha256Before": hashlib.sha256(image_before).hexdigest(), "pngSha256Hover": hashlib.sha256(image_hover).hexdigest()}, ensure_ascii=False, indent=2), encoding="utf-8")
        page.screenshot(path=str(output / "countdown-zero.png"))
        page.reload(wait_until="networkidle")
        is_eula(page)
        expect(page.get_by_role("checkbox")).to_have_attribute("aria-checked", "false")
        check(snapshot(page)["record"] == {}, "Refusal saved a restart bypass")

    @case("reject-wins-concurrent-accept", {"delays": {"onboarding_reject": 550}})
    def reject_wins(page, output):
        is_eula(page)
        page.get_by_role("checkbox").click()
        page.evaluate("""() => {
            const buttons = [...document.querySelectorAll('.ni-modal__foot button')];
            const reject = buttons.find(b => b.textContent.includes('\u62d2\u7edd'));
            const accept = buttons.find(b => b.textContent.includes('\u540c\u610f\u5e76\u7b7e\u7f72'));
            for (let i = 0; i < 15; i++) reject.dispatchEvent(new MouseEvent('click', {bubbles: true}));
            accept.dispatchEvent(new MouseEvent('click', {bubbles: true}));
        }""")
        expect(page.get_by_role("dialog", name=EXIT)).to_be_visible()
        check(len(calls(snapshot(page), "onboarding_reject")) == 1 and not calls(snapshot(page), "onboarding_accept_eula"), "Reject and accept IPC raced")

    @case("strictmode-stale-read-ignored", {"responses": {"onboarding_get": [{"value": {"eulaVersion": REVISION, "eulaUrl": "https://www.minecraft.net/zh-hans/eula", "eulaAcceptedAt": 1700000000, "currentVersion": VERSION, "needsEula": False, "needsChangelog": False, "exiting": False}, "delayMs": 350}]}})
    def stale_read(page, output):
        is_eula(page)
        page.wait_for_timeout(600)
        is_eula(page)
        check(not calls(snapshot(page), "onboarding_finish"), "Stale StrictMode read completed startup")
        check(len(calls(snapshot(page), "onboarding_get")) == 2, "Fixture did not exercise StrictMode effect replay")

    @case("strictmode-finish-latest-callback", {"record": COMPLETE, "holdCommands": ["onboarding_finish"]})
    def latest_callback(page, output):
        page.wait_for_function("window.__startupFixture.snapshot().events.some(e => e.command === 'onboarding_finish')")
        page.evaluate("window.__startupFixture.rerender()")
        page.wait_for_timeout(100)
        check(snapshot(page)["readyCount"] == 0, "Deferred finish unexpectedly resolved")
        page.evaluate("window.__startupFixture.release('onboarding_finish')")
        wait_ready(page)
        check(snapshot(page)["readyGenerations"] == [1], "Finish promise called a stale onReady callback")
        check(len(calls(snapshot(page), "onboarding_finish")) == 1, "Callback change repeated finish IPC")

    @case("strictmode-finish-unmount-cleanup", {"record": COMPLETE, "holdCommands": ["onboarding_finish"]})
    def finish_unmount(page, output):
        page.wait_for_function("window.__startupFixture.snapshot().events.some(e => e.command === 'onboarding_finish')")
        page.evaluate("window.__startupFixture.hideGate()")
        page.wait_for_function("window.__startupFixture.snapshot().dialogs === 0")
        page.evaluate("window.__startupFixture.release('onboarding_finish')")
        page.wait_for_timeout(200)
        state = snapshot(page)
        check(state["readyCount"] == 0 and state["dialogs"] == 0, "Unmounted gate updated parent after finish")
        check(state["modalListeners"].get("keydown", 0) == 0 and state["modalListeners"].get("focusin", 0) == 0, "Modal listener leaked after unmount")

    @case("strictmode-refusal-unmount-cleanup")
    def refusal_unmount(page, output):
        is_eula(page)
        page.get_by_role("button", name=REJECT, exact=True).click()
        expect(page.get_by_role("dialog", name=EXIT)).to_be_visible()
        state = snapshot(page)
        check(state["activeIntervals"] == 1 and state["modalListeners"] == {"keydown": 1, "focusin": 1}, "StrictMode duplicated refusal listeners or timers")
        page.evaluate("window.__startupFixture.hideGate()")
        page.wait_for_timeout(100)
        state = snapshot(page)
        check(state["activeIntervals"] == 0 and state["modalListeners"] == {"keydown": 0, "focusin": 0}, "Unmount leaked interval or modal handlers")
        page.wait_for_function("window.__startupFixture.snapshot().exitCount === 1", timeout=6000)
        state = snapshot(page)
        check(not calls(state, "window_close") and state["readyCount"] == 0, "Unmounted frontend issued a late close or ready callback")

    @case("already-exiting-status-fails-closed", {"record": COMPLETE, "exiting": True})
    def already_exiting(page, output):
        expect(page.get_by_role("dialog", name=EXIT)).to_be_visible()
        check(not calls(snapshot(page), "onboarding_finish") and snapshot(page)["shellInert"], "Exiting status enabled startup")

    @case("invalid-read-response-fails-closed", {"responses": {"onboarding_get": [{"value": {"currentVersion": VERSION}}, {"value": {"currentVersion": VERSION}}]}})
    def invalid_read(page, output):
        expect(page.get_by_role("alert")).to_be_visible()
        check(not calls(snapshot(page), "onboarding_finish"), "Malformed status reached finish IPC")
        check(snapshot(page)["shellInert"], "Malformed status enabled shell")

    @case("release-mismatch-read-retry", {"responses": {"onboarding_get": [{"value": {"currentVersion": "1.0.3"}}, {"value": {"currentVersion": "1.0.3"}}]}})
    def mismatch(page, output):
        expect(page.get_by_role("alert")).to_contain_text("\u66f4\u65b0\u8bf4\u660e\u4e0e\u7a0b\u5e8f\u7248\u672c\u4e0d\u4e00\u81f4")
        page.get_by_role("button", name=RETRY, exact=True).click()
        is_eula(page)

    @case("release-mismatch-accept-fails-closed", {"responses": {"onboarding_accept_eula": [{"value": {"eulaVersion": REVISION, "eulaUrl": "https://www.minecraft.net/zh-hans/eula", "eulaAcceptedAt": 1700000000, "currentVersion": "1.0.3", "needsEula": False, "needsChangelog": True, "exiting": False}}]}})
    def accept_mismatch(page, output):
        is_eula(page)
        page.get_by_role("checkbox").click()
        page.get_by_role("button", name=ACCEPT, exact=True).click()
        expect(page.get_by_role("alert")).to_contain_text("\u66f4\u65b0\u8bf4\u660e\u4e0e\u7a0b\u5e8f\u7248\u672c\u4e0d\u4e00\u81f4")
        is_eula(page)
        check(not calls(snapshot(page), "onboarding_ack_changelog"), "Mismatched accept status reached release acknowledgment")

    @case("preview-first-run-and-restart", {"transport": "preview"})
    def preview_first(page, output):
        is_eula(page)
        sign(page)
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)
        record = json.loads(snapshot(page)["previewRecord"])
        check(record["eulaVersion"] == REVISION and record["seenVersion"] == VERSION, "Preview acceptance did not persist")
        check(not any(entry["kind"] == "ipc" for entry in snapshot(page)["events"]), "Preview path issued desktop IPC")
        page.reload(wait_until="networkidle")
        wait_ready(page)

    @case("preview-null-record-recovers", {"transport": "preview", "previewRaw": "null"})
    def preview_null(page, output):
        is_eula(page)
        check(snapshot(page)["shellInert"], "Null preview record bypassed EULA")

    @case("preview-string-timestamp-reasks", {"transport": "preview", "previewRecord": {"eulaVersion": REVISION, "acceptedAt": "1700000000", "seenVersion": VERSION}})
    def preview_timestamp(page, output):
        is_eula(page)
        expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_disabled()

    @case("preview-fractional-timestamp-reasks", {"transport": "preview", "previewRecord": {"eulaVersion": REVISION, "acceptedAt": 1.5, "seenVersion": VERSION}})
    def preview_fraction(page, output):
        is_eula(page)

    @case("preview-storage-write-failure", {"transport": "preview", "denyPreviewStorage": "set"})
    def preview_write(page, output):
        is_eula(page)
        page.get_by_role("checkbox").click()
        page.get_by_role("button", name=ACCEPT, exact=True).click()
        expect(page.get_by_role("alert")).to_be_visible()
        is_eula(page)
        check(snapshot(page)["record"] == {} and snapshot(page)["shellInert"], "Denied preview storage bypassed consent")

    @case("production-app-shell-and-portable-order", {"mode": "app", "portablePending": True, "autoLaunch": True})
    def app_shell(page, output):
        is_eula(page)
        check(snapshot(page)["shellInert"], "Production App shell is not inert during EULA")
        check(not calls(snapshot(page), "portable_bootstrap_status"), "Production App mounted portable bootstrap before EULA")
        non_dismiss(page)
        sign(page)
        check(snapshot(page)["shellInert"] and not calls(snapshot(page), "portable_bootstrap_status"), "Production App unlocked before changelog")
        page.get_by_role("button", name=ACK, exact=True).click()
        wait_ready(page)
        page.wait_for_function("window.__startupFixture.snapshot().events.some(e => e.command === 'portable_bootstrap_start')")
        check(snapshot(page)["autoLaunchCount"] == 1, "Production App did not dispatch gated startup")
        page.get_by_role("dialog", name="\u6b63\u5728\u51c6\u5907\u4fbf\u643a\u6574\u5408\u5305").get_by_role("button", name="\u540e\u53f0\u7ee7\u7eed", exact=True).click()
        page.get_by_role("button", name="\u8bbe\u7f6e", exact=True).click()
        expect(page.locator('.ni-navtab[data-active="true"]')).to_have_text("\u8bbe\u7f6e")
        check(not calls(snapshot(page), "window_close"), "Window close was invoked while startup was gated")
        page.screenshot(path=str(output / "production-shell.png"))

    with sync_playwright() as playwright:
        launch = {"headless": True}
        if args.executable_path:
            if not args.executable_path.is_file():
                raise ValueError("Browser executable does not exist")
            launch["executable_path"] = str(args.executable_path.resolve())
        else:
            launch["channel"] = args.channel
        browser = playwright.chromium.launch(**launch)
        browser_version = browser.version
        try:
            for name, setup, function in cases:
                output = args.evidence_dir / name
                output.mkdir()
                context = browser.new_context(viewport={"width": 1100, "height": 720}, device_scale_factor=1, reduced_motion="no-preference")
                context.add_init_script("window.__STARTUP_SETUP__ = " + json.dumps(setup, ensure_ascii=True) + ";")
                page = context.new_page()
                page.set_default_timeout(6500)
                errors, console = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("console", lambda message: console.append({"type": message.type, "text": message.text}))
                entry = {"case": name, "scope": "production-components-simulated-ipc" if setup.get("transport") != "preview" else "production-components-browser-preview-storage", "nativeAcceptance": False, "startedAt": datetime.now(timezone.utc).isoformat()}
                started = time.monotonic()
                try:
                    page.goto(args.base_url + "/", wait_until="networkidle")
                    page.wait_for_function("Boolean(window.__startupFixture)")
                    page.evaluate("document.fonts.ready")
                    page.screenshot(path=str(output / "initial.png"))
                    function(page, output)
                    check(not errors, "Uncaught browser errors: " + repr(errors))
                    entry["passed"] = True
                except Exception as error:
                    entry.update({"passed": False, "error": str(error), "traceback": traceback.format_exc()})
                finally:
                    entry["durationSeconds"] = round(time.monotonic() - started, 3)
                    try:
                        entry["finalSnapshot"] = snapshot(page)
                        page.screenshot(path=str(output / "final.png"))
                    except Exception as error:
                        entry["evidenceError"] = str(error)
                    entry["pageErrors"] = errors
                    entry["console"] = console
                    (output / "result.json").write_text(json.dumps(entry, ensure_ascii=False, indent=2), encoding="utf-8")
                    (output / "console.json").write_text(json.dumps(console, ensure_ascii=False, indent=2), encoding="utf-8")
                    results.append(entry)
                    print(("PASS " if entry["passed"] else "FAIL ") + name + " (" + str(entry["durationSeconds"]) + "s)", flush=True)
                    if not entry["passed"]:
                        print(entry.get("error", ""), flush=True)
                    context.close()
        finally:
            browser.close()
    report = {"scope": "headless-production-browser-components", "nativeAcceptance": False, "ipc": "simulated; never a native backend", "browserChannel": args.channel, "browserVersion": browser_version, "executablePath": str(args.executable_path.resolve()) if args.executable_path else None, "clock": "real clocks and real timer scheduling; only cleanup instrumentation, no clock acceleration", "caseCount": len(results), "passedCount": sum(entry["passed"] for entry in results), "passed": bool(results) and all(entry["passed"] for entry in results), "results": results, "omissions": ["Real release window and process exit are not exercised here.", "Automatic launch dispatch is simulated; no Minecraft process is started."]}
    (args.evidence_dir / "browser-results.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Browser evidence: " + str((args.evidence_dir / "browser-results.json").resolve()), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
