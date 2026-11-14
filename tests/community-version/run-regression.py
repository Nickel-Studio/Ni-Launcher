"""使用已安装 Chrome/Edge 验证真实社区页面；不安装浏览器或依赖。"""
import argparse
import json
from pathlib import Path

from playwright.sync_api import sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument("--url", default="http://127.0.0.1:1523/tests/community-version/regression.html")
parser.add_argument("--output", default="design/community-version/browser-results.json")
args = parser.parse_args()
output = Path(args.output)
output.parent.mkdir(parents=True, exist_ok=True)

with sync_playwright() as playwright:
    errors = []
    browser = None
    for channel in ("chrome", "msedge"):
        try:
            browser = playwright.chromium.launch(channel=channel, headless=True)
            break
        except Exception as error:
            errors.append(str(error))
    if browser is None:
        raise RuntimeError("未能启动系统浏览器：\n" + "\n".join(errors))
    try:
        page = browser.new_page(viewport={"width": 1100, "height": 900})
        console_errors = []
        page.on("pageerror", lambda error: console_errors.append(str(error)))
        page.on("console", lambda message: console_errors.append(message.text) if message.type == "error" else None)
        page.goto(args.url, wait_until="networkidle")
        page.wait_for_function("document.documentElement.dataset.regression === 'pass' || document.documentElement.dataset.regression === 'fail'", timeout=30000)
        report = page.evaluate("window.__communityVersionRegression")
        report["consoleErrors"] = console_errors
        report["browser"] = browser.version
        page.screenshot(path=str(output.with_suffix(".png")), full_page=True)
        output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False, indent=2))
        assert report["status"] == "pass", "社区版本浏览器回归失败"
        assert not console_errors, "浏览器控制台存在错误"
    finally:
        browser.close()
