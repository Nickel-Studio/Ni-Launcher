"""Real browser assertions over production instance UI with synthetic IPC only."""
import argparse
import json
import traceback
from pathlib import Path
from playwright.sync_api import sync_playwright

DEFAULT = "X:/nickel-order-fixture/default"
OLDER = "X:/nickel-order-fixture/z-old"
YOUNGER = "X:/nickel-order-fixture/a-new"
LATEST = "X:/nickel-order-fixture/b-last"
PORTABLE = "X:/nickel-order-fixture/portable"
MISSING = "X:/nickel-order-fixture/missing"
DEFAULT_NAME = "\u9ed8\u8ba4\u5b9e\u4f8b\u76ee\u5f55"
ADD_NAME = "\u6dfb\u52a0\u5df2\u6709\u6587\u4ef6\u5939"
REMOVE_NAME = "\u79fb\u51fa\u5217\u8868"
LABELS = {OLDER: "Oldest", YOUNGER: "Younger"}


def seed(roots=None, game=None, default=DEFAULT, screen="nav", **extra):
    return {
        "registry": {"roots": roots if roots is not None else [OLDER, DEFAULT, YOUNGER], "active": YOUNGER, "labels": LABELS},
        "gameDir": game if game is not None else YOUNGER,
        "defaultDir": default,
        "screen": screen,
        **extra,
    }


class Case:
    def __init__(self, page, url, output):
        self.page = page
        self.url = url
        self.output = output
        self.checks = []
        self.snapshots = []

    def snapshot(self):
        return self.page.evaluate("window.__rootOrder.snapshot()")

    def ready(self):
        self.page.wait_for_function("""() => {
          const s = window.__rootOrder?.snapshot();
          return s && s.mounted && (s.screen === 'nav'
            ? s.hook && !s.hook.loading && !s.hook.busy
            : !s.ui.loading && s.ui.names.length > 0 && !s.ui.disabled.some(Boolean));
        }""", timeout=12000)
        self.page.wait_for_load_state("networkidle")

    def open(self):
        self.page.goto(self.url, wait_until="networkidle")
        self.ready()

    def call(self, name, *args):
        self.page.evaluate("([name, args]) => window.__rootOrder[name](...args)", [name, list(args)])
        self.ready()

    def check(self, name, actual, expected):
        ok = actual == expected
        self.checks.append({"name": name, "ok": ok, "actual": actual, "expected": expected})
        if not ok:
            raise AssertionError(f"{name}: expected {expected!r}, got {actual!r}")

    def checkpoint(self, name, roots, names, active_name=None):
        self.ready()
        snap = self.snapshot()
        self.snapshots.append({"name": name, "value": snap})
        if snap["hook"]:
            self.check(f"{name}: hook roots", snap["hook"]["roots"], roots)
            self.check(f"{name}: hook error", snap["hook"]["error"], "")
        self.check(f"{name}: visible sidebar names", snap["ui"]["names"], names)
        self.check(f"{name}: visible sidebar paths", snap["ui"]["paths"], roots)
        self.check(f"{name}: row visibility", [r["visible"] for r in snap["ui"]["rows"]], [True] * len(roots))
        ys = [r["y"] for r in snap["ui"]["rows"]]
        self.check(f"{name}: visual order follows DOM", ys == sorted(ys) and len(set(ys)) == len(ys), True)
        self.check(f"{name}: unexpected IPC", snap["unexpected"], [])
        self.check(f"{name}: fixture errors", snap["errors"], [])
        self.check(f"{name}: page errors", snap["ui"]["errors"], [])
        if active_name is not None:
            self.check(f"{name}: single active sidebar row", snap["ui"]["activeNames"], [active_name])
        return snap

    def persisted(self, name, roots, active=None):
        snap = self.snapshot()
        self.check(f"{name}: persisted roots", snap["disk"]["registry"]["roots"], roots)
        if active is not None:
            self.check(f"{name}: persisted registry active", snap["disk"]["registry"]["active"], active)
            self.check(f"{name}: persisted game directory", snap["disk"]["game"]["gameDir"], active)

    def select(self, label):
        row = self.page.locator(".vs-nav__item").filter(has=self.page.locator(".vs-nav__name", has_text=label))
        row.click()
        self.ready()

    def add(self, directory, full_page=False):
        if full_page:
            self.page.evaluate("dir => window.__rootOrder.pick(dir)", directory)
        else:
            self.page.get_by_label("Synthetic directory", exact=True).fill(directory)
        self.page.get_by_role("button", name=ADD_NAME, exact=True).click()
        self.ready()

    def remove(self, label):
        row = self.page.locator(".vs-nav__row").filter(has=self.page.locator(".vs-nav__name", has_text=label))
        row.locator(".vs-nav__remove").click()
        self.page.get_by_role("menuitem", name=REMOVE_NAME, exact=True).click()
        self.ready()

    def shot(self, name):
        target = self.output / f"{name}.png"
        self.page.screenshot(path=str(target), full_page=True)
        return str(target)


def ordering_flow(c):
    c.open()
    roots = [DEFAULT, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.checkpoint("legacy migration", roots, names, "Younger")
    c.persisted("legacy migration writes once", roots, YOUNGER)
    c.select("Oldest")
    c.checkpoint("activate older without reordering", roots, names, "Oldest")
    c.persisted("activation preserves disk order", roots, OLDER)
    c.select("Younger")
    c.checkpoint("activate younger without reordering", roots, names, "Younger")
    c.add(LATEST)
    roots = [DEFAULT, OLDER, YOUNGER, LATEST]
    names = [DEFAULT_NAME, "Oldest", "Younger", "b-last"]
    c.checkpoint("new addition at end", roots, names, "b-last")
    c.persisted("addition persists", roots, LATEST)
    writes_before = sum(call["command"] == "instances_set" for call in c.snapshot()["calls"])
    c.call("reload")
    c.checkpoint("hook reload", roots, names, "b-last")
    writes_after = sum(call["command"] == "instances_set" for call in c.snapshot()["calls"])
    c.check("stable reload does not rewrite registry", writes_after, writes_before)
    c.page.evaluate("window.__rootOrder.unmount()")
    c.page.evaluate("window.__rootOrder.mount('nav')")
    c.checkpoint("hook remount", roots, names, "b-last")
    c.page.reload(wait_until="networkidle")
    c.checkpoint("document restart", roots, names, "b-last")
    c.persisted("restart persists", roots, LATEST)
    c.remove("Oldest")
    c.checkpoint("remove older", [DEFAULT, YOUNGER, LATEST], [DEFAULT_NAME, "Younger", "b-last"], "b-last")
    c.add(OLDER)
    roots = [DEFAULT, YOUNGER, LATEST, OLDER]
    names = [DEFAULT_NAME, "Younger", "b-last", "Oldest"]
    c.checkpoint("remove then readd receives new position", roots, names, "Oldest")
    c.persisted("readdition persists", roots, OLDER)
    c.shot("readded-order")
    c.remove("Oldest")
    c.checkpoint("remove active uses first root without reordering", [DEFAULT, YOUNGER, LATEST], [DEFAULT_NAME, "Younger", "b-last"], DEFAULT_NAME)
    c.persisted("active removal persists", [DEFAULT, YOUNGER, LATEST], DEFAULT)


def alias_flow(c):
    c.open()
    roots = [DEFAULT, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.checkpoint("aliases collapse and active/default share retained identity", roots, names, "Oldest")
    c.persisted("alias migration persists canonical active", roots, OLDER)
    alias = "x:\\NICKEL-ORDER-FIXTURE\\Z-OLD\\\\"
    c.call("addRoot", alias)
    c.checkpoint("adding alias does not duplicate or move", roots, names, "Oldest")
    c.persisted("alias activation stores retained spelling", roots, OLDER)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("alias restart", roots, names, "Oldest")
    c.call("removeRoot", alias)
    c.checkpoint("alias removal removes physical identity", [DEFAULT, YOUNGER], [DEFAULT_NAME, "Younger"], DEFAULT_NAME)
    c.persisted("alias removal persists", [DEFAULT, YOUNGER], DEFAULT)
    c.add(OLDER)
    c.checkpoint("readding alias identity is appended", [DEFAULT, YOUNGER, OLDER], [DEFAULT_NAME, "Younger", "Oldest"], "Oldest")
    c.shot("alias-order")


def activation_alias(c):
    c.open()
    roots = [DEFAULT, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.call("activate", "x:\\nickel-order-fixture\\A-NEW\\")
    c.checkpoint("direct activation alias", roots, names, "Younger")
    c.persisted("direct alias activation uses existing root", roots, YOUNGER)
    c.page.evaluate("dir => window.__rootOrder.references(dir)", "x:\\nickel-order-fixture\\A-NEW\\")
    c.check("alias selection references initialized", c.snapshot()["references"]["launch"] is not None and c.snapshot()["references"]["focus"] is not None, True)
    c.call("removeRoot", "X:\\NICKEL-ORDER-FIXTURE\\a-new\\")
    c.checkpoint("direct removal alias", [DEFAULT, OLDER], [DEFAULT_NAME, "Oldest"], DEFAULT_NAME)
    c.check("alias removal clears matching launch and focus", c.snapshot()["references"], {"launch": None, "focus": None})


def portable_flow(c):
    c.open()
    roots = [PORTABLE, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.checkpoint("effective portable default and valid roots", roots, names, DEFAULT_NAME)
    c.persisted("portable alias migration", roots, PORTABLE)
    c.select("Oldest")
    c.checkpoint("portable activation cannot reorder roots", roots, names, "Oldest")
    c.add(LATEST)
    roots = [PORTABLE, OLDER, YOUNGER, LATEST]
    names = [DEFAULT_NAME, "Oldest", "Younger", "b-last"]
    c.checkpoint("portable new valid root appended", roots, names, "b-last")
    c.persisted("portable addition persists", roots, LATEST)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("portable restart retains effective valid root order", roots, names, "b-last")
    c.check("missing portable root remains absent", MISSING in c.snapshot()["ui"]["paths"], False)
    c.remove("Oldest")
    c.checkpoint("portable removal does not replace game active with registry mirror", [PORTABLE, YOUNGER, LATEST], [DEFAULT_NAME, "Younger", "b-last"], "b-last")
    c.persisted("portable inactive removal persists", [PORTABLE, YOUNGER, LATEST], LATEST)
    c.add(OLDER)
    roots = [PORTABLE, YOUNGER, LATEST, OLDER]
    names = [DEFAULT_NAME, "Younger", "b-last", "Oldest"]
    c.checkpoint("portable removal and readdition", roots, names, "Oldest")
    c.page.reload(wait_until="networkidle")
    c.checkpoint("portable readdition restart", roots, names, "Oldest")
    c.shot("portable-order")


def versions_flow(c):
    c.open()
    roots = [DEFAULT, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.checkpoint("real Versions page legacy sidebar", roots, names, "Younger")
    c.select("Oldest")
    c.checkpoint("real Versions sidebar activation", roots, names, "Oldest")
    c.add(LATEST, full_page=True)
    c.checkpoint("real Versions directory picker addition", [DEFAULT, OLDER, YOUNGER, LATEST], [DEFAULT_NAME, "Oldest", "Younger", "b-last"], "b-last")
    c.persisted("real Versions addition persists", [DEFAULT, OLDER, YOUNGER, LATEST], LATEST)
    c.remove("Oldest")
    c.add(OLDER, full_page=True)
    roots = [DEFAULT, YOUNGER, LATEST, OLDER]
    names = [DEFAULT_NAME, "Younger", "b-last", "Oldest"]
    c.checkpoint("real Versions remove/readd", roots, names, "Oldest")
    c.page.reload(wait_until="networkidle")
    c.checkpoint("real Versions restart", roots, names, "Oldest")
    c.persisted("real Versions restart persists", roots, OLDER)
    c.shot("versions-sidebar")


def empty_flow(c):
    c.open()
    c.checkpoint("empty registry initializes default", [DEFAULT], [DEFAULT_NAME], DEFAULT_NAME)
    c.persisted("empty default registered", [DEFAULT], DEFAULT)
    c.add(OLDER)
    c.add(YOUNGER)
    c.checkpoint("empty then sequential additions", [DEFAULT, OLDER, YOUNGER], [DEFAULT_NAME, "z-old", "a-new"], "a-new")


def missing_active_flow(c):
    c.open()
    roots = [DEFAULT, OLDER, YOUNGER, LATEST]
    names = [DEFAULT_NAME, "Oldest", "Younger", "b-last"]
    c.checkpoint("unregistered game directory appended, not promoted", roots, names, "b-last")
    c.persisted("unregistered game directory persists", roots, LATEST)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("unregistered active restart", roots, names, "b-last")


def helper_flow(c):
    c.open()
    tests = [
        ([OLDER, DEFAULT, YOUNGER, LATEST], DEFAULT, [DEFAULT, OLDER, YOUNGER, LATEST]),
        ([OLDER, YOUNGER], DEFAULT, [OLDER, YOUNGER]),
        ([OLDER, OLDER.upper().replace("/", "\\") + "\\", DEFAULT], DEFAULT, [DEFAULT, OLDER]),
        (["\\\\?\\X:\\nickel-order-fixture\\z-old", OLDER, DEFAULT], DEFAULT, [DEFAULT, "\\\\?\\X:\\nickel-order-fixture\\z-old"]),
        (["//server/share/z-old", "\\\\SERVER\\SHARE\\Z-OLD\\", DEFAULT], DEFAULT, [DEFAULT, "//server/share/z-old"]),
        (["//server/share/z-old", "\\\\?\\UNC\\SERVER\\SHARE\\Z-OLD\\", DEFAULT], DEFAULT, [DEFAULT, "//server/share/z-old"]),
        (["//server/share/z-old", "\\\\?\\unc\\server\\share\\default\\", "//SERVER/SHARE/DEFAULT"], "//server/share/default", ["\\\\?\\unc\\server\\share\\default\\", "//server/share/z-old"]),
        (["", " ", OLDER, DEFAULT], DEFAULT, [DEFAULT, OLDER]),
        ([OLDER, DEFAULT], "x:\\NICKEL-ORDER-FIXTURE\\DEFAULT\\", [DEFAULT, OLDER]),
    ]
    for index, (roots, default, expected) in enumerate(tests):
        actual = c.page.evaluate("([roots, defaultDir]) => window.__rootOrder.order(roots, defaultDir)", [roots, default])
        c.check(f"production order helper example {index}", actual["roots"], expected)
        c.check(f"production order helper example {index} input unchanged", actual["inputUnchanged"], True)
    for index in range(40):
        others = [f"X:/order-fixture/{(index * 7 + j * 11) % 101:03d}" for j in range(6)]
        roots = others[:index % 7] + [DEFAULT] + others[index % 7:]
        actual = c.page.evaluate("([roots, defaultDir]) => window.__rootOrder.order(roots, defaultDir)", [roots, DEFAULT])
        c.check(f"production helper stable order property {index}", actual["roots"], [DEFAULT] + others)
        c.check(f"production helper purity property {index}", actual["inputUnchanged"], True)


def default_readdition_flow(c):
    c.open()
    c.checkpoint("default readdition initial", [DEFAULT, OLDER, YOUNGER], [DEFAULT_NAME, "Oldest", "Younger"], "Younger")
    c.remove(DEFAULT_NAME)
    c.checkpoint("default removed without moving survivors", [OLDER, YOUNGER], ["Oldest", "Younger"], "Younger")
    alias = "x:\\NICKEL-ORDER-FIXTURE\\DEFAULT\\"
    c.add(alias)
    retained = alias.rstrip("\\")
    roots = [retained, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.checkpoint("default alias readdition is the first-root exception", roots, names, DEFAULT_NAME)
    c.persisted("default alias readdition persists", roots, retained)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("default alias readdition restart", roots, names, DEFAULT_NAME)
    c.shot("default-readdition")


def no_default_flow(c):
    c.open()
    c.checkpoint("default metadata unavailable retains old order", [OLDER, YOUNGER], ["Oldest", "Younger"], "Younger")
    c.add(LATEST)
    roots = [OLDER, YOUNGER, LATEST]
    names = ["Oldest", "Younger", "b-last"]
    c.checkpoint("without default new root is still appended", roots, names, "b-last")
    c.page.reload(wait_until="networkidle")
    c.checkpoint("without default restart", roots, names, "b-last")
    c.persisted("without default persistence", roots, LATEST)


def legacy_objects_flow(c):
    c.open()
    roots = [DEFAULT, OLDER, YOUNGER]
    names = [DEFAULT_NAME, "Oldest", "Younger"]
    c.checkpoint("legacy object roots migrate without reordering", roots, names, "Younger")
    c.persisted("legacy object roots persist as strings", roots, YOUNGER)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("legacy object roots restart", roots, names, "Younger")


def delayed_activation_flow(c):
    c.open()
    c.checkpoint("delayed activation initial", [DEFAULT, OLDER, YOUNGER], [DEFAULT_NAME, "Oldest", "Younger"], "Younger")
    c.page.evaluate("window.__rootOrder.holdGameWrite()")
    before = sum(call["command"] == "game_settings_set" for call in c.snapshot()["calls"])
    c.page.evaluate("dir => { window.__delayedRootAction = window.__rootOrder.activate(dir); }", OLDER)
    c.page.wait_for_function("before => window.__rootOrder.snapshot().calls.filter(call => call.command === 'game_settings_set').length > before", arg=before)
    c.page.evaluate("dir => window.__rootOrder.externalAppend(dir)", LATEST)
    c.check("external addition exists while game write is pending", c.snapshot()["disk"]["registry"]["roots"], [DEFAULT, OLDER, YOUNGER, LATEST])
    c.page.evaluate("window.__rootOrder.releaseGameWrite()")
    c.page.evaluate("window.__delayedRootAction")
    roots = [DEFAULT, OLDER, YOUNGER, LATEST]
    names = [DEFAULT_NAME, "Oldest", "Younger", "b-last"]
    c.checkpoint("delayed activation preserves latest registry additions", roots, names, "Oldest")
    c.persisted("delayed activation order persists", roots, OLDER)



def delayed_alias_alignment_flow(c):
    c.open()
    c.checkpoint("alias alignment initial", [DEFAULT, OLDER, YOUNGER], [DEFAULT_NAME, "Oldest", "Younger"], "Younger")
    c.page.evaluate("window.__rootOrder.holdGameWrite()")
    before = sum(call["command"] == "game_settings_set" for call in c.snapshot()["calls"])
    alias = LATEST.upper().replace("/", "\\") + "\\"
    c.page.evaluate("dir => { window.__delayedRootAction = window.__rootOrder.activate(dir); }", alias)
    c.page.wait_for_function("before => window.__rootOrder.snapshot().calls.filter(call => call.command === 'game_settings_set').length > before", arg=before)
    c.page.evaluate("dir => window.__rootOrder.externalAppend(dir)", LATEST)
    c.page.evaluate("() => { window.__rootOrder.releaseGameWrite(); window.__rootOrder.holdGameWrite(); }")
    c.page.wait_for_function("before => window.__rootOrder.snapshot().calls.filter(call => call.command === 'game_settings_set').length > before + 1", arg=before)
    newest = "X:/nickel-order-fixture/c-after"
    c.page.evaluate("dir => window.__rootOrder.externalAppend(dir)", newest)
    roots = [DEFAULT, OLDER, YOUNGER, LATEST, newest]
    c.check("external addition exists during alias alignment write", c.snapshot()["disk"]["registry"]["roots"], roots)
    c.page.evaluate("window.__rootOrder.releaseGameWrite()")
    c.page.evaluate("window.__delayedRootAction")
    c.checkpoint("alias alignment preserves latest registry additions", roots, [DEFAULT_NAME, "Oldest", "Younger", "b-last", "c-after"], "b-last")
    c.persisted("aligned alias and complete order persist", roots, LATEST)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("aligned alias restart keeps additions", roots, [DEFAULT_NAME, "Oldest", "Younger", "b-last", "c-after"], "b-last")



def extended_unc_flow(c):
    c.open()
    default = "//server/share/default"
    older = "//server/share/z-old"
    younger = "//server/share/a-new"
    roots = [default, older, younger]
    names = [DEFAULT_NAME, "z-old", "a-new"]
    c.checkpoint("extended UNC aliases migrate in addition order", roots, names, "z-old")
    c.persisted("extended UNC migration persists unique identities", roots, older)
    c.call("activate", "\\\\?\\unc\\SERVER\\SHARE\\A-NEW\\")
    c.checkpoint("extended UNC activation retains order", roots, names, "a-new")
    c.persisted("extended UNC activation aligns stored identity", roots, younger)
    c.call("references", "\\\\?\\UNC\\SERVER\\SHARE\\Z-OLD\\")
    c.call("removeRoot", "\\\\?\\unc\\server\\share\\z-old\\")
    c.checkpoint("extended UNC removal retains survivors", [default, younger], [DEFAULT_NAME, "a-new"], "a-new")
    c.check("extended UNC removal clears alias references", c.snapshot()["references"], {"launch": None, "focus": None})
    c.call("addRoot", older)
    roots = [default, younger, older]
    names = [DEFAULT_NAME, "a-new", "z-old"]
    c.checkpoint("extended UNC readdition takes new position", roots, names, "z-old")
    c.persisted("extended UNC readdition order persists", roots, older)
    c.page.reload(wait_until="networkidle")
    c.checkpoint("extended UNC restart retains new order", roots, names, "z-old")




def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--only", default="")
    args = parser.parse_args()
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    url = args.base + "/tests/version-instances/root-order.html"
    aliases = [OLDER, OLDER.upper().replace("/", "\\") + "\\", DEFAULT, YOUNGER]
    unc_roots = ["//server/share/z-old", "//server/share/default", "//server/share/a-new", "\\\\?\\UNC\\SERVER\\SHARE\\Z-OLD\\", "\\\\?\\UNC\\SERVER\\SHARE\\DEFAULT\\"]
    cases = [
        ("helper-properties", seed(), helper_flow),
        ("legacy-lifecycle", seed(), ordering_flow),
        ("path-alias-migration", seed(aliases, game=OLDER.upper().replace("/", "\\") + "\\", default=DEFAULT.lower().replace("/", "\\") + "\\"), alias_flow),
        ("direct-alias-actions", seed(), activation_alias),
        ("portable-effective-roots", seed([OLDER, MISSING, YOUNGER, PORTABLE.upper().replace("/", "\\") + "\\"], game=PORTABLE, default=PORTABLE, portable=True, validDirs=[OLDER, YOUNGER, LATEST, PORTABLE.upper().replace("/", "\\") + "\\"]), portable_flow),
        ("production-Versions-page", seed(screen="versions"), versions_flow),
        ("empty-registry", {**seed(), "registry": {"roots": [], "active": None, "labels": {}}, "gameDir": ""}, empty_flow),
        ("unregistered-active", seed(game=LATEST), missing_active_flow),
        ("default-alias-readdition", seed(), default_readdition_flow),
        ("missing-default-metadata", seed([OLDER, YOUNGER], default=""), no_default_flow),
        ("legacy-object-roots", seed([{"dir": OLDER + "/"}, DEFAULT, {"dir": YOUNGER}, OLDER.upper().replace("/", "\\") + "\\", "", None], game=YOUNGER), legacy_objects_flow),
        ("delayed-activation-keeps-additions", seed(), delayed_activation_flow),
        ("delayed-alias-alignment-keeps-additions", seed(), delayed_alias_alignment_flow),
        ("extended-unc-alias-lifecycle", seed(unc_roots, game="\\\\?\\UNC\\server\\share\\z-old\\", default="\\\\?\\unc\\SERVER\\SHARE\\DEFAULT\\"), extended_unc_flow),
    ]
    if args.only:
        wanted = set(args.only.split(","))
        cases = [case for case in cases if case[0] in wanted]
    results = []
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel="msedge", headless=True, args=["--no-first-run", "--no-default-browser-check"])
        try:
            for name, initial, action in cases:
                target = output / name
                target.mkdir(parents=True, exist_ok=True)
                context = browser.new_context(viewport={"width": 1180, "height": 900}, reduced_motion="reduce")
                context.add_init_script("window.__rootOrderSeed = " + json.dumps(initial) + ";")
                page = context.new_page()
                console_errors = []
                page.on("pageerror", lambda error: console_errors.append(str(error)))
                page.on("console", lambda message: console_errors.append(message.text) if message.type == "error" else None)
                case = Case(page, url, target)
                result = {"name": name, "status": "pass", "scope": "Real system Edge, production UI, mocked IPC, synthetic paths and persistence; not native acceptance", "seed": initial}
                try:
                    action(case)
                    case.check("browser console has no errors", console_errors, [])
                    result["final"] = case.snapshot()
                    if name == "legacy-lifecycle":
                        storage = context.storage_state()
                        context.close()
                        context = browser.new_context(viewport={"width": 1180, "height": 900}, reduced_motion="reduce", storage_state=storage)
                        page = context.new_page()
                        case.page = page
                        page.on("pageerror", lambda error: console_errors.append(str(error)))
                        page.on("console", lambda message: console_errors.append(message.text) if message.type == "error" else None)
                        case.open()
                        case.checkpoint("fresh browser context restart", [DEFAULT, YOUNGER, LATEST], [DEFAULT_NAME, "Younger", "b-last"], DEFAULT_NAME)
                        case.persisted("fresh context disk order", [DEFAULT, YOUNGER, LATEST], DEFAULT)
                        result["freshContextFinal"] = case.snapshot()
                        case.check("fresh context console has no errors", console_errors, [])
                except Exception as error:
                    result["status"] = "fail"
                    result["error"] = str(error)
                    result["traceback"] = traceback.format_exc()
                    try:
                        result["failureState"] = case.snapshot()
                        result["failureScreenshot"] = case.shot("failure")
                    except Exception:
                        pass
                finally:
                    result["checks"] = case.checks
                    result["snapshots"] = case.snapshots
                    result["consoleErrors"] = console_errors
                    (target / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
                    results.append(result)
                    context.close()
        finally:
            browser.close()
    report = {
        "status": "pass" if results and all(r["status"] == "pass" for r in results) else "fail",
        "scope": "Production hook/navigation and Versions page in system Edge with explicitly mocked IPC. Synthetic data only. No native acceptance.",
        "cases": [{"name": r["name"], "status": r["status"], "passed": sum(c["ok"] for c in r["checks"]), "failed": sum(not c["ok"] for c in r["checks"]), "error": r.get("error"), "evidence": str(output / r["name"] / "result.json")} for r in results],
        "passed": sum(c["ok"] for r in results for c in r["checks"]),
        "failed": sum(not c["ok"] for r in results for c in r["checks"]),
        "browserVersion": browser.version,
    }
    (output / "browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
