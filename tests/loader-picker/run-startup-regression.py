"""启动与安装布局回归；仅使用已安装的 Python Playwright 与系统 Chrome/Edge。

先执行 pnpm dev -- --host 127.0.0.1 --port 1531，再运行：
python tests/loader-picker/run-startup-regression.py --base-url http://127.0.0.1:1531
截图和可量测结果写入 design/startup-loading，不下载任何浏览器。
"""
import argparse
import json
from pathlib import Path

from playwright.sync_api import sync_playwright


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--base-url", default="http://127.0.0.1:1531")
parser.add_argument("--channel", choices=("chrome", "msedge"), default="chrome")
parser.add_argument("--output", default="design/startup-loading")
args = parser.parse_args()
output = Path(args.output)
output.mkdir(parents=True, exist_ok=True)
ipc_script = Path(__file__).with_name("startup-ipc.js").read_text(encoding="utf-8")
checks = []
screenshots = []


def check(name, condition, detail=None):
    checks.append({"name": name, "ok": bool(condition), "detail": detail})
    print(f'{"PASS" if condition else "FAIL"} {name}', flush=True)
    if not condition:
        raise AssertionError(f"{name}: {detail}")


def screenshot(page, name):
    target = output / f"{name}.png"
    page.screenshot(path=str(target))
    screenshots.append(str(target))


def settle(page, selector):
    page.locator(selector).evaluate("""async (element) => {
      const animations = element.getAnimations({subtree: true})
        .filter((animation) => Number.isFinite(animation.effect.getTiming().iterations));
      await Promise.all(animations.map((animation) => animation.finished.catch(() => {})));
    }""")


def new_app(browser, **config):
    options = {"mode": "dark", "settings": "ready", "manifest": "ready", "catalog": "short", "splash": "normal"}
    options.update(config)
    page = browser.new_page(
        viewport={"width": 900, "height": 600},
        reduced_motion=options.get("motion", "no-preference"),
        color_scheme=options.get("system", "dark"),
    )
    page.set_default_timeout(10000)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.add_init_script("window.__startupConfig = " + json.dumps(options) + ";\n" + ipc_script)
    page.goto(args.base_url, wait_until="networkidle")
    page.wait_for_function("window.__startupRegression?.times.removed !== undefined")
    times = page.evaluate("window.__startupRegression.times")
    startup_inert = page.evaluate("window.__startupRegression.appInertDuringStartup")
    app_after = page.evaluate("window.__startupRegression.appAfterReveal")
    check("启动遮挡期间整个应用（含 body 级弹窗）不可聚焦", startup_inert is True, startup_inert)
    check(
        "交接后整个应用移除 inert",
        bool(app_after)
        and not app_after["bodyInert"]
        and not app_after["rootInert"]
        and not app_after["shellInert"],
        app_after,
    )
    if options["splash"] == "hidden":
        # 窗口在加载期间不可见：不能提前放行外壳/删除闪屏，必须等真实可见信号。
        # 用替身记录的 shown（窗口转为可见的时刻）判定，避免固定阈值在 CI 上抖动假失败。
        check(
            "隐藏窗口不提前放行首屏",
            "shown" in times and times["ready"] >= times["shown"] - 20,
            times,
        )
        check(
            "窗口可见后才淡出闪屏",
            times["removed"] - times["ready"] < 700,
            times,
        )
    elif options["splash"] == "stuck":
        # 窗口可见性查询永久悬挂：3 秒兜底只解锁外壳，6 秒硬截止必须强制交接。
        # leaving 记录淡出开始时刻，确保不是提前淡出后才移除。
        check(
            "可见性查询悬挂时硬截止仍移除闪屏",
            not page.locator("#nw-splash").count()
            and times.get("leaving", 0) - times["commit"] >= 5800
            and times["removed"] >= times.get("leaving", 0),
            times,
        )
        app = page.evaluate("__startupRegression.appAfterReveal")
        check(
            "硬截止后应用恢复指针交互并移除 inert",
            bool(app)
            and app["shellPointerEvents"] == "auto"
            and not app["bodyInert"]
            and not app["rootInert"]
            and not app["shellInert"],
            app,
        )
    else:
        check("React 提交即放行首屏", times["ready"] - times["commit"] < 200, times)
    if options["splash"] == "missing-event":
        check("事件缺失才使用退出兜底", 700 < times["removed"] - times["ready"] < 1300, times)
    elif options["splash"] == "stuck":
        # 硬截止路径的退出时序已在上面单独断言，不再套用「提交即就绪」的短耗时预期。
        pass
    elif options.get("motion") == "reduce" or options["splash"] in ("no-transition", "cancel"):
        check("无动效或取消时直接移除 Splash", times["removed"] - times["ready"] < 100, times)
    else:
        check("正常退出不等待固定停留或兜底", times["removed"] - times["ready"] < 700, times)
        check("真实 opacity transitionend 驱动退出", "transitionEnd" in times, times)
    if options["splash"] == "child-transition":
        check("忽略子元素与非 opacity 过渡", page.evaluate("__startupRegression.ignoredUnrelatedTransitions"))
    check("启动没有未捕获异常", not errors, errors)
    return page


def open_resources(page):
    page.get_by_role("button", name="资源", exact=True).click()
    page.locator(".van-page").wait_for(state="visible")
    settle(page, ".nw-content")


def open_picker(page):
    open_resources(page)
    page.locator(".van-latest__card").first.click()
    page.locator(".van-picker__download").wait_for(state="visible")
    settle(page, ".nw-content")


def footer_metrics(page):
    return page.evaluate("""() => {
      const host = document.querySelector('.nw-pager__page');
      const picker = document.querySelector('.van-picker');
      const footer = picker.querySelector('.van-picker__foot');
      const button = footer.querySelector('.van-picker__download');
      const rect = button.getBoundingClientRect();
      const style = getComputedStyle(picker);
      const content = [...picker.children].filter((element) => element !== footer);
      return {
        count: picker.querySelectorAll('.van-picker__download').length,
        last: picker.lastElementChild === footer,
        position: getComputedStyle(footer).position,
        paddingBottom: parseFloat(style.paddingBottom),
        host: host.getBoundingClientRect().toJSON(),
        picker: picker.getBoundingClientRect().toJSON(),
        footer: footer.getBoundingClientRect().toJSON(),
        button: rect.toJSON(),
        previousBottom: Math.max(...content.map((element) => element.getBoundingClientRect().bottom)),
        hit: button.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2)),
        overflowX: host.scrollWidth > host.clientWidth,
      };
    }""")


def check_footer(page, name):
    metrics = footer_metrics(page)
    check(name + "主操作只有一个且位于最后", metrics["count"] == 1 and metrics["last"], metrics)
    check(name + "主操作在正常滚动流，不遮挡内容", metrics["position"] == "static" and metrics["previousBottom"] <= metrics["footer"]["top"] + 1, metrics)
    check(name + "900x600 主操作完整可见可点", metrics["button"]["top"] >= metrics["host"]["top"] and metrics["button"]["bottom"] <= metrics["host"]["bottom"] + 1 and metrics["hit"] and not metrics["overflowX"], metrics)
    check(name + "胶囊底部对齐内容区", abs(metrics["host"]["bottom"] - metrics["button"]["bottom"] - metrics["paddingBottom"]) < 2, metrics)


def mining_metrics(page):
    return page.evaluate("""() => {
      const loading = document.querySelector('.van-page--loading .nw-loading');
      const card = loading.querySelector('.nw-loading__card');
      const clip = loading.querySelector('.nw-loading__clip');
      const art = loading.querySelector('svg');
      const host = document.querySelector('.nw-pager__page');
      const header = document.querySelector('.van-page__head');
      return {
        loading: loading.getBoundingClientRect().toJSON(),
        card: card.getBoundingClientRect().toJSON(),
        clip: clip.getBoundingClientRect().toJSON(),
        art: art.getBoundingClientRect().toJSON(),
        host: host.getBoundingClientRect().toJSON(),
        header: header.getBoundingClientRect().toJSON(),
        gap: parseFloat(getComputedStyle(loading.parentElement).rowGap),
        background: getComputedStyle(card).backgroundColor,
        animations: [...loading.querySelectorAll('svg g, svg path')].map((node) => getComputedStyle(node).animationName),
      };
    }""")


def check_mining(page):
    metrics = mining_metrics(page)
    center_y = (metrics["header"]["bottom"] + metrics["gap"] + metrics["host"]["bottom"]) / 2
    card = metrics["card"]
    loading = metrics["loading"]
    check("加载卡片在页头以下垂直居中", abs(card["y"] + card["height"] / 2 - center_y) < 2, metrics)
    check("动画与加载卡片水平居中", abs(card["x"] + card["width"] / 2 - (loading["x"] + loading["width"] / 2)) < 1 and abs(metrics["clip"]["x"] + metrics["clip"]["width"] / 2 - (card["x"] + card["width"] / 2)) < 1, metrics)
    check("恢复只去顶部30单位空白的裁切取舍", abs(metrics["clip"]["height"] / metrics["clip"]["width"] - 114 / 160) < 0.001 and abs(metrics["clip"]["y"] - metrics["art"]["y"] - 30) < 1, metrics)
    return metrics


with sync_playwright() as playwright:
    browser = playwright.chromium.launch(channel=args.channel, headless=True)
    try:
        for settings_state in ("ready", "pending", "failed"):
            page = new_app(browser, settings=settings_state)
            screenshot(page, "startup-" + settings_state)
            page.close()
        for splash_state in ("missing-event", "no-transition", "hidden", "cancel", "child-transition"):
            page = new_app(browser, splash=splash_state)
            page.close()
        page = new_app(browser, motion="reduce")
        shell = page.locator(".nw-shell")
        shell.evaluate("element => element.classList.add('nw-shell--leaving')")
        reduced_exit = shell.evaluate("element => ({transition: getComputedStyle(element).transitionDuration, transform: getComputedStyle(element).transform})")
        check("reduced-motion 关闭外壳退出缩放与过渡", reduced_exit["transition"] == "0s" and reduced_exit["transform"] == "none", reduced_exit)
        page.close()

        backgrounds = []
        for mode, system, motion in (("dark", "dark", "no-preference"), ("light", "light", "no-preference"), ("system", "light", "reduce")):
            page = new_app(browser, mode=mode, system=system, motion=motion, manifest="pending")
            open_resources(page)
            metrics = check_mining(page)
            backgrounds.append(metrics["background"])
            check("明暗模式解析正确", page.evaluate("document.documentElement.dataset.theme") == (system if mode == "system" else mode))
            if motion == "reduce":
                check("reduced-motion 停止全部挖矿循环", all(name == "none" for name in metrics["animations"]), metrics["animations"])
            else:
                bounds = page.locator(".nw-loading__art").evaluate("""(svg) => {
                  const clip = svg.parentElement.getBoundingClientRect();
                  const pick = svg.querySelector('.nw-loading__pick');
                  const animation = pick.getAnimations()[0];
                  animation.pause();
                  const bounds = [];
                  for (const progress of [0, 0.12, 0.32, 0.44, 0.54, 0.7, 1]) {
                    animation.currentTime = animation.effect.getTiming().duration * progress;
                    const box = pick.getBoundingClientRect();
                    bounds.push({top: box.top >= clip.top, left: box.left >= clip.left, right: box.right <= clip.right, bottom: box.bottom <= clip.bottom});
                  }
                  animation.play();
                  return bounds;
                }""")
                check("顶部裁切不切断完整挥动轨迹", all(all(bound.values()) for bound in bounds), bounds)
            screenshot(page, "loading-900-" + mode)
            page.close()
        check("加载卡片明暗模式填充不同", backgrounds[0] != backgrounds[1], backgrounds)

        for mode in ("dark", "light"):
            page = new_app(browser, mode=mode)
            open_picker(page)
            page.wait_for_function("document.querySelectorAll('.van-loader').length === 0")
            check_footer(page, "短内容")
            screenshot(page, "cta-short-900-" + mode)
            page.close()

        page = new_app(browser, catalog="long")
        open_picker(page)
        page.locator(".van-loader[data-loader='forge'] .van-loader__head").click()
        page.locator(".van-builds--all .van-build").last.wait_for(state="attached")
        settle(page, ".nw-content")
        listing = page.locator(".van-loader[data-loader='forge'] .van-builds--all")
        listing.evaluate("element => element.scrollTop = element.scrollHeight")
        page.locator(".nw-pager__page").evaluate("element => element.scrollTop = element.scrollHeight")
        page.locator(".van-picker__download").focus()
        check_footer(page, "长内容")
        check("长构建列表末行可滚动到达", listing.evaluate("element => element.scrollTop + element.clientHeight >= element.scrollHeight - 1"))
        check("底部按钮保留键盘焦点", page.locator(".van-picker__download").evaluate("element => element === document.activeElement && getComputedStyle(element).boxShadow !== 'none'"))
        screenshot(page, "cta-long-900-dark")
        page.close()

        page = new_app(browser, catalog="pending")
        open_picker(page)
        page.locator(".nw-pager__page").evaluate("element => element.scrollTop = element.scrollHeight")
        check_footer(page, "加载器查询中")
        page.locator(".van-picker__download").click()
        page.wait_for_function("__startupRegression.calls.some(call => call.command === 'install_version')")
        install = page.evaluate("__startupRegression.calls.filter(call => call.command === 'install_version')")
        check("加载器查询不阻塞原版安装且只创建一次", len(install) == 1 and install[0]["args"]["loader"] == "" and install[0]["args"]["loaderVersion"] is None, install)
        page.close()

        for route, state_name in (("loader-picker", "__loaderPickerRegression"), ("ui-foundation", "__uiFoundationRegression")):
            for motion in ("no-preference", "reduce"):
                page = browser.new_page(viewport={"width": 900, "height": 600}, reduced_motion=motion)
                page.goto(f"{args.base_url}/tests/{route}/regression.html", wait_until="networkidle")
                page.wait_for_function(f"window.{state_name}?.status !== 'running'", timeout=20000)
                state = page.evaluate(f"window.{state_name}")
                check(f"既有 {route} 回归 {motion}", state["status"] == "pass", {"checks": len(state["checks"]), "failed": [item for item in state["checks"] if not item["ok"]]})
                page.close()

        # 6 秒硬截止会显著拉长耗时，放在最后，避免影响其它快速场景的定位。
        page = new_app(browser, splash="stuck")
        screenshot(page, "startup-stuck-visibility")
        page.close()
    except Exception as error:
        checks.append({"name": "回归执行异常", "ok": False, "detail": str(error)})
        raise
    finally:
        browser.close()
        report = {"status": "pass" if checks and all(item["ok"] for item in checks) else "fail", "checks": checks, "screenshots": screenshots}
        (output / "regression-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
print(f"PASS {len(checks)} checks; evidence: {output}", flush=True)
