"""Exercise the real export panel with mock IPC and a locally installed browser."""
import argparse
import json
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument("--url", default="http://127.0.0.1:1537/tests/export-regression/regression.html")
parser.add_argument("--output", default=None)
args = parser.parse_args()
checks = []
errors = []


def check(name, condition):
    assert condition, name + (": " + " | ".join(errors) if errors else "")
    checks.append(name)


with sync_playwright() as playwright:
    browser = None
    launch_errors = []
    for channel in ("chrome", "msedge", None):
        try:
            browser = playwright.chromium.launch(channel=channel, headless=True)
            break
        except Exception as error:
            launch_errors.append(str(error))
    if browser is None:
        raise RuntimeError("No local browser available: " + "\n".join(launch_errors))
    try:
        page = browser.new_page(viewport={"width": 1100, "height": 900})
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.on("console", lambda message: errors.append(message.text) if message.type == "error" else None)
        page.goto(args.url, wait_until="networkidle")
        expect(page.locator(".vst-export")).to_be_visible()
        expect(page.get_by_role("button", name="保存配置", exact=True)).to_be_enabled()
        check("Collapsed groups mount no physical file or package rows", page.locator(".vst-export__file").count() == 0)
        check("Preview contains 267 compact entries rather than package internals", page.evaluate("window.__exportRegression.preview.files.length") == 267)
        check("All nineteen categories render without a separate disabled Mod category", page.locator(".vst-export__group").count() == 19 and page.get_by_role("checkbox", name="导出已禁用的 Mod", exact=True).count() == 0)
        check("Directory sizes are deferred until export", "完整大小在导出时计算" in page.locator(".vst-export__footer").inner_text())
        for label in ("游戏本体个人信息", "多人游戏服务器列表", "JEI 个人数据", "EMI 个人数据", "帕秋莉个人数据", "已绘制的个人地图", "截图", "导出的结构", "录像回放与视频", "单人游戏存档"):
            expect(page.get_by_role("checkbox", name="导出" + label, exact=True)).to_have_attribute("aria-checked", "false")
        checks.append("All personal categories are unchecked by default")

        resources = page.locator(".vst-export__group-toggle").filter(has_text="资源包")
        resources.click()
        expect(page.locator(".vst-export__file")).to_have_count(48)
        check("Resource packages render flattened display names", page.get_by_role("checkbox", name="导出Emerald Workshop", exact=True).count() == 1)
        check("Resource pack internals do not render", "assets/" not in page.locator(".vst-export__files").inner_text())
        page.get_by_role("button", name="下一页", exact=True).click()
        expect(page.locator(".vst-export__file")).to_have_count(48)
        page.get_by_role("button", name="下一页", exact=True).click()
        expect(page.locator(".vst-export__file")).to_have_count(5)
        check("ZIP packs stay a single preview entry", page.get_by_role("checkbox", name="导出Archive pack", exact=True).count() == 1)
        resources.click()
        expect(page.locator(".vst-export__file")).to_have_count(0)
        checks.append("Expanded groups are paginated to at most 48 rows and unmount on collapse")

        mods = page.locator(".vst-export__group-toggle").filter(has_text="Mod")
        mods = mods.filter(has_not_text="设置")
        mods.click()
        expect(page.locator(".vst-export__file")).to_have_count(48)
        expect(page.get_by_role("checkbox", name="导出disabled-fixture.jar.disabled", exact=True)).to_have_attribute("aria-checked", "false")
        expect(page.get_by_role("checkbox", name="导出Nested disabled mods", exact=True)).to_have_attribute("aria-checked", "false")
        checks.append("Disabled Mod files and directory tokens remain opt-in inside Mod")
        mods.click()
        expect(page.locator(".vst-export__file")).to_have_count(0)
        checks.append("Large Mod groups are lazy and paginated")

        warning = page.locator(".vst-export__resource-warning")
        bundle = page.get_by_role("switch", name="导出资源文件", exact=True)
        expect(warning).to_have_count(0)
        bundle.click()
        expect(warning).to_be_visible()
        expect(warning.locator(".ni-inline-notice__title")).to_have_text("注意")
        expected_notice = "建议仅在无法稳定连接CurseForge或Modrinth 时才打包资源文件。\n公开发布含资源文件的整合包可能会违反部分Mod的使用协议。"
        check("Warning body is exactly the approved two-line notice", warning.locator(".ni-inline-notice__message").text_content() == expected_notice)
        check("Warning is inline and yellow", warning.get_attribute("data-tone") == "warning" and warning.locator(".ni-inline-notice__title").evaluate("node => { const probe = document.createElement('span'); probe.style.color = 'var(--warning)'; node.append(probe); const expected = getComputedStyle(probe).color; probe.remove(); return getComputedStyle(node).color === expected; }") )
        bundle.click()
        expect(warning).to_have_count(0)
        checks.append("Resource warning appears and disappears with its switch")

        old_paths = ["screenshots/one.png", "config/jei/world/private/bookmarks.ini", "saves/Selected/level.dat", "journeymap/data/sp/world/map.dat"]
        def load_config(paths, bundle_resources, format="mrpack", launcher=False, personalization=False):
            page.evaluate("config => { window.__exportRegression.loadedConfig = config; }", {
                "name": "Saved Fixture", "version": "2.0", "files": paths,
                "bundleResources": bundle_resources, "modrinthOnly": True, "format": format,
                "includeLauncher": launcher, "includePersonalization": personalization,
            })
            page.get_by_role("button", name="读取配置", exact=True).click()
            expect(page.get_by_role("button", name="保存配置", exact=True)).to_be_enabled()
            expect(bundle).to_have_attribute("aria-checked", str(bundle_resources).lower())

        def save_config():
            count = page.evaluate("window.__exportRegression.savedConfigs.length")
            page.get_by_role("button", name="保存配置", exact=True).click()
            page.wait_for_function("previous => window.__exportRegression.savedConfigs.length > previous", arg=count)
            expect(page.get_by_role("button", name="保存配置", exact=True)).to_be_enabled()
            return page.evaluate("window.__exportRegression.savedConfigs.at(-1)")

        load_config(old_paths, True)
        expect(warning).to_be_visible()
        for label in ("截图", "JEI 个人数据", "单人游戏存档", "已绘制的个人地图"):
            expect(page.get_by_role("checkbox", name="导出" + label, exact=True)).to_have_attribute("aria-checked", "mixed")
        check("Loading old files does not mount recursive lists", page.locator(".vst-export__file").count() == 0)
        saved = save_config()
        check("Saving legacy partial selections preserves exact individual files", sorted(saved["files"]) == sorted(old_paths))
        check("Saving configs omits request identity and preview metadata", set(saved) == {"name", "version", "files", "bundleResources", "modrinthOnly", "format", "includeLauncher", "includePersonalization"})
        checks.append("Saved bundleResources true immediately synchronizes the inline notice")

        page.get_by_role("checkbox", name="导出截图", exact=True).click()
        expect(page.get_by_role("checkbox", name="导出截图", exact=True)).to_have_attribute("aria-checked", "true")
        saved = save_config()
        check("Explicitly checking a partial directory replaces only its legacy files", "@dir/screenshots/screenshots" in saved["files"] and "screenshots/one.png" not in saved["files"] and all(path in saved["files"] for path in old_paths[1:]))
        load_config(old_paths, False)
        expect(warning).to_have_count(0)
        checks.append("Loading a saved bundleResources false hides the notice")

        disabled_paths = ["mods/disabled-fixture.jar.disabled", "mods/nested/deeper.jar.disabled"]
        load_config(disabled_paths, False)
        expect(page.get_by_role("checkbox", name="导出Mod", exact=True)).to_have_attribute("aria-checked", "mixed")
        check("Legacy disabled Mod files retain exact paths in the shared category", save_config()["files"] == disabled_paths)
        load_config(["@dir/disabledMods/mods/nested"], False)
        check("Existing disabled directory tokens round-trip without namespace changes", save_config()["files"] == ["@dir/disabledMods/mods/nested"])

        many = [f"screenshots/{index}.png" for index in range(10_000)]
        load_config(many, False)
        expect(page.get_by_role("checkbox", name="导出截图", exact=True)).to_have_attribute("aria-checked", "mixed")
        check("Ten thousand legacy screenshot files remain a single UI bucket", page.locator(".vst-export__file").count() == 0 and "已选 1 项" in page.locator(".vst-export__footer").inner_text())
        check("Ten thousand legacy files survive configuration round-trip", save_config()["files"] == many)

        load_config(["@dir/resourcepacks/resourcepacks/Pack-0"], True, "portable", True, True)
        expect(page.get_by_role("switch", name="附带 Nickel 启动器", exact=True)).to_have_attribute("aria-checked", "true")
        expect(page.get_by_role("switch", name="附带个性化设置", exact=True)).to_have_attribute("aria-checked", "true")
        saved = save_config()
        check("Portable ZIP retains launcher and personalization options", saved["format"] == "portable" and saved["includeLauncher"] and saved["includePersonalization"])
        check("One selected resource package excludes its siblings", saved["files"] == ["@dir/resourcepacks/resourcepacks/Pack-0"])
        page.locator('input[type="radio"][value="mrpack"]').check()
        saved = save_config()
        check("Switching to standard mrpack clears portable-only export flags", saved["format"] == "mrpack" and not saved["includeLauncher"] and not saved["includePersonalization"])

        check("Resource warning never opens a modal or dialog", page.get_by_role("dialog").count() == 0 and not page.evaluate("window.__exportRegression.calls.some(call => call.command.includes('dialog'))"))
        check("Browser console has no errors", not errors)
        report = {"status": "pass", "checks": checks, "consoleErrors": errors, "browser": browser.version}
        if args.output:
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
            page.screenshot(path=str(output.with_suffix(".png")), full_page=True)
        print(json.dumps(report, ensure_ascii=False, indent=2))
    finally:
        browser.close()
