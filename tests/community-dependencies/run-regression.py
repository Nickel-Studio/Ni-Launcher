import argparse
import base64
import datetime
import json
import math
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument("--url", default="http://127.0.0.1:1567/tests/community-dependencies/regression.html")
parser.add_argument("--output", default=None)
parser.add_argument("--inspect", action="store_true")
args = parser.parse_args()
output = Path(args.output or f"tests/community-dependencies/evidence/browser-{datetime.datetime.now().strftime('%Y%m%d-%H%M%S')}")
output.mkdir(parents=True, exist_ok=False)
report = {"status": "running", "checks": [], "focusCases": [], "sessions": [], "screenshots": [], "errors": []}


def check(name, condition, detail=None):
    report["checks"].append({"name": name, "ok": bool(condition), "detail": detail})
    if not condition:
        raise AssertionError(f"{name}: {detail}")


def screenshot(page, name):
    path = output / f"{name}.png"
    image = page.screenshot(path=str(path), full_page=False, animations="disabled")
    report["screenshots"].append(str(path))
    return image


def navigate(page, url):
    # Vite dependency optimization can keep requests open after React is ready.
    page.goto(url, wait_until="domcontentloaded")
    page.wait_for_function("window.__communityDependencies && document.querySelector('.resources-browser')", timeout=90000)
    expect(page.get_by_role("button", name="Mod", exact=True)).to_be_visible(timeout=90000)
    page.evaluate("""async () => {
      await document.fonts.ready;
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    }""")


def active(page):
    return page.locator(".resources-detail:not([hidden])")


def panel(page, version_id):
    return active(page).locator(f'.pd-deps[data-version-id="{version_id}"]')


def wait_ready(page, version_id):
    page.wait_for_function("""id => {
      const frame = document.querySelector('.resources-detail:not([hidden])');
      const block = frame?.querySelector(`.pd-deps[data-version-id="${id}"]`);
      return block?.querySelector('.pd-deps__toggle')?.getAttribute('aria-expanded') === 'true'
        && block?.querySelector('.pd-deps__body')?.getAttribute('aria-busy') === 'false';
    }""", arg=version_id)


def select_option(page, selector, label):
    page.locator(selector).click()
    page.get_by_role("option", name=label, exact=True).click()
    page.wait_for_timeout(450)


def open_mod(page):
    page.get_by_role("button", name="Mod", exact=True).click()
    expect(page.get_by_role("searchbox", name="搜索Mod")).to_be_visible()
    expect(page.get_by_role("combobox", name="MC 版本", exact=True)).to_have_value("1.21.1")
    page.wait_for_function("document.querySelectorAll('.mr-card__body').length >= 2")
    page.wait_for_timeout(450)


def open_project(page, title):
    page.get_by_role("button", name=f"查看 {title} 的版本", exact=True).click()
    expect(active(page).locator(".pd-card__title")).to_have_text(title)
    expect(active(page).locator(".pd-group").first).to_be_visible()
    page.wait_for_timeout(100)


def back(page, nested=True):
    active(page).get_by_role("button", name="返回上级资源" if nested else "返回检索", exact=True).click()
    page.wait_for_timeout(150)


def expand_dependencies(page, version_id):
    block = panel(page, version_id)
    trigger = block.locator(".pd-deps__toggle")
    if trigger.get_attribute("aria-expanded") != "true":
        trigger.click()
    wait_ready(page, version_id)
    return block


def show_all_groups(page):
    all_pill = active(page).get_by_role("button", name="全部", exact=True)
    if all_pill.count() and all_pill.get_attribute("aria-pressed") != "true":
        all_pill.click()
    page.wait_for_timeout(100)


def expand_history(page):
    group = active(page).locator(".pd-group").filter(has=page.locator(".pd-group__title", has_text="1.20.1"))
    if group.count() and group.locator(".pd-group__toggle").get_attribute("aria-expanded") != "true":
        group.locator(".pd-group__toggle").click()
    page.wait_for_timeout(100)


def save_version(page, version_id, mode="cancel"):
    page.evaluate("mode => window.__communityDependencies.setSaveMode(mode)", mode)
    before = page.evaluate("window.__communityDependencies.saves.length")
    row = active(page).locator(f'.pd-row[data-version-id="{version_id}"]').first
    expect(row).to_be_enabled()
    row.click()
    page.wait_for_function("count => window.__communityDependencies.saves.length > count", arg=before)
    expect(row).to_be_enabled()
    return page.evaluate("window.__communityDependencies.saves.at(-1)")


def frame_state(page, remember=False):
    return page.evaluate("""remember => {
      const node = document.querySelector('.resources-detail:not([hidden])');
      if (remember) window.__rememberedDetail = node;
      return {
        id: node.dataset.detailId,
        sameDOM: node === window.__rememberedDetail,
        filter: node.querySelector('.pd-pill[aria-pressed="true"]')?.textContent,
        groups: [...node.querySelectorAll('.pd-group')].map(group => [group.querySelector('.pd-group__title').textContent, group.dataset.open]),
        dependencies: [...node.querySelectorAll('.pd-deps__toggle')].map(toggle => [toggle.getAttribute('aria-label'), toggle.getAttribute('aria-expanded')]),
        top: document.querySelector('.nw-content').scrollTop,
        focus: document.activeElement?.getAttribute('aria-label')
      };
    }""", remember)


def compare_frame_state(name, before, after):
    check(name, after["sameDOM"] and before["id"] == after["id"] and before["filter"] == after["filter"]
          and before["groups"] == after["groups"] and before["dependencies"] == after["dependencies"]
          and abs(before["top"] - after["top"]) <= 2 and before["focus"] == after["focus"], {"before": before, "after": after})


def search_state(page, remember=False):
    return page.evaluate("""remember => {
      const host = document.querySelector('.resources-browser');
      if (remember) window.__rememberedSearch = host;
      return {
        sameDOM: host === window.__rememberedSearch,
        query: host.querySelector('input[type="search"]').value,
        mcVersion: host.querySelector('input[aria-label="MC 版本"]').value,
        loader: host.querySelector('.cv-loader .nw-select__trigger').textContent,
        source: host.querySelector('.cv-toolbar__source .nw-select__trigger').textContent,
        page: host.querySelector('.cv-pager__page')?.textContent,
        top: host.querySelector('.nw-pager__page').scrollTop,
        results: [...host.querySelectorAll('.mr-card__name')].map(node => node.textContent),
        focus: document.activeElement?.getAttribute('aria-label')
      };
    }""", remember)


def no_auto_actions(page, name, saves_before):
    calls = page.evaluate("window.__communityDependencies.calls")
    check(name, not any(call["command"] in ("modrinth_install", "curseforge_install", "pack_install", "modpack_install") for call in calls)
          and page.evaluate("window.__communityDependencies.saves.length") == saves_before)


def attach_session(page, name):
    page.set_default_timeout(15000)
    page.set_default_navigation_timeout(90000)
    session = {"name": name, "consoleErrors": [], "pageErrors": [], "remoteRequests": []}
    page.on("pageerror", lambda error: session["pageErrors"].append(str(error)))
    page.on("console", lambda message: session["consoleErrors"].append(message.text) if message.type == "error" else None)
    page.on("request", lambda request: session["remoteRequests"].append(request.url) if request.url.startswith("https://") else None)
    report["sessions"].append(session)
    return session


def functional(page):
    open_mod(page)
    check("合并检索保留两源相同 slug 的独立项目", page.get_by_role("button", name="查看 Modrinth dependency regression 的版本", exact=True).count() == 1
          and page.get_by_role("button", name="查看 CurseForge dependency regression 的版本", exact=True).count() == 1)
    search = page.get_by_role("searchbox", name="搜索Mod")
    search.fill("Nickel retained search")
    mc = page.get_by_role("combobox", name="MC 版本", exact=True)
    mc.fill("1.20.1")
    mc.press("Enter")
    select_option(page, ".cv-loader .nw-select__trigger", "NeoForge")
    select_option(page, ".cv-toolbar__source .nw-select__trigger", "Modrinth")
    page.wait_for_function("window.__communityDependencies.calls.filter(c => c.command === 'modrinth_search').at(-1)?.args.query === 'Nickel retained search'")
    page.get_by_role("button", name="下一页", exact=True).click()
    expect(page.locator(".cv-pager__page")).to_have_text("2")
    page.wait_for_timeout(450)
    card = page.get_by_role("button", name="查看 Modrinth dependency regression 的版本", exact=True)
    card.scroll_into_view_if_needed()
    card.focus()
    page.locator(".nw-pager__page").evaluate("node => { node.scrollTop = 160; }")
    saved_search = search_state(page, True)
    check("检索保留状态验收使用非零滚动与第二页", saved_search["top"] > 0 and saved_search["page"] == "2", saved_search)
    card.click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Modrinth dependency regression")
    expect(active(page).locator('.pd-row[data-version-id="MainMRCurrent"]')).to_be_visible()
    check("必要前置折叠时不发解析或下载请求", not page.evaluate("window.__communityDependencies.calls.some(c => c.command === 'community_dependency_resolve')"))
    show_all_groups(page)
    expand_history(page)
    block = expand_dependencies(page, "MainMRCurrent")
    check("Modrinth 仅必要关系且保留 Fabric 与 Quilt API", block.get_by_role("button", name="打开前置 Fabric API 的详情", exact=True).count() == 1
          and block.get_by_role("button", name="打开前置 Quilt API 的详情", exact=True).count() == 1
          and not any(label in block.inner_text() for label in ("OptionalMR", "IncompatibleMR", "EmbeddedMR", "Historical dependency only")))
    check("Modrinth 重复声明、别名、自依赖去重后仍保留同名不同项目与不同固定版本", block.locator(".pd-deps__item").count() == 11
          and block.get_by_role("button", name="打开前置 Same named required project 的详情", exact=True).count() == 2
          and block.get_by_role("button", name="打开前置 Pinned dependency 的详情", exact=True).count() == 2)
    check("仅文件名回退保留手动提示且没有可猜测的详情入口", block.locator(".pd-deps__unresolved").filter(has_text="manual-required.jar").count() == 1
          and block.locator(".pd-deps__unresolved").filter(has_text="manual-quilt.jar").count() == 1
          and "仅提供文件名，无法定位项目" in block.inner_text())
    check("项目读取失败、固定版本被换成 latest、项目身份不符均显示可重试错误", block.get_by_role("button", name="重试读取前置 项目 UnstableMR", exact=True).count() == 1
          and "返回的前置版本与声明不一致" in block.inner_text()
          and "指定前置版本不属于返回的项目" in block.inner_text())
    screenshot(page, "modrinth-required-before-retry")
    tasks_before = page.evaluate("window.__communityDependencies.taskCount()")
    cancelled = save_version(page, "MainMRCurrent")
    check("前置网络失败不阻止主资源另存为取消且不创建任务", cancelled["taskId"] is None and page.evaluate("window.__communityDependencies.taskCount()") == tasks_before
          and cancelled["args"]["versionId"] == "MainMRCurrent")
    saved = save_version(page, "MainMRCurrent", "save")
    check("前置失败时主资源仍走原另存为保存流程", saved["taskId"] is not None and saved["args"]["slugOrId"] == "ParentMR"
          and saved["args"]["instanceVersionId"] == "fixture-instance"
          and saved["args"]["dir"] == "C:/NickelDependencyFixture")
    block.get_by_role("button", name="重试读取前置 项目 UnstableMR", exact=True).click()
    wait_ready(page, "MainMRCurrent")
    check("重试恢复三个失败前置，不替换指定版本或丢失同名项目", block.locator(".pd-deps__project").count() == 9
          and block.locator('[role="alert"]').count() == 0 and block.locator(".pd-deps__item").count() == 11)
    history = expand_dependencies(page, "MainMRHistory")
    check("Modrinth 历史版本只展示它自己的必要前置", history.locator(".pd-deps__item").count() == 1
          and "Historical dependency only" in history.inner_text() and "Fabric API" not in history.inner_text()
          and "Historical dependency only" not in block.inner_text())
    exact_link = block.get_by_role("button", name="打开前置 Pinned dependency 的详情", exact=True).filter(has_text="要求版本 2.0.0")
    exact_link.scroll_into_view_if_needed()
    exact_link.focus()
    parent_state = frame_state(page, True)
    check("父详情状态验收使用非零外层滚动", parent_state["top"] > 0, parent_state)
    saves_before = page.evaluate("window.__communityDependencies.saves.length")
    exact_link.click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Pinned dependency")
    expect(active(page).locator('.pd-row[data-required="true"]')).to_have_attribute("data-version-id", "ExactMR1")
    check("固定版本历史读取失败仍保留原指定版本可下载", "History metadata network failure" in active(page).inner_text()
          and "2.0.0" in active(page).locator(".pd-dependency-context").inner_text()
          and active(page).locator('.pd-row[data-version-id="LatestMR"]').count() == 0)
    no_auto_actions(page, "点击必要前置只导航详情，不自动安装或打开另存为", saves_before)
    cancelled_exact = save_version(page, "ExactMR1")
    check("固定前置另存为严格使用 ExactMR1 而非最新版本", cancelled_exact["args"]["versionId"] == "ExactMR1"
          and cancelled_exact["args"]["slugOrId"] == "PinnedMR" and cancelled_exact["taskId"] is None)
    active(page).locator(".pd-error").get_by_role("button", name="重试", exact=True).click()
    page.wait_for_function("document.querySelector('.resources-detail:not([hidden]) .pd-error') === null")
    latest_group = active(page).locator(".pd-group").filter(has=page.locator(".pd-group__title", has_text="1.21.1"))
    latest_group.locator(".pd-group__toggle").scroll_into_view_if_needed()
    if latest_group.locator(".pd-group__toggle").get_attribute("aria-expanded") != "true":
        latest_group.locator(".pd-group__toggle").click()
    expect(latest_group.locator('.pd-row[data-version-id="LatestMR"]')).to_be_visible()
    check("历史重试后指定旧版本置顶且新版本仍保留供手动选择", active(page).locator(".pd-group").first.locator(".pd-row").first.get_attribute("data-version-id") == "ExactMR1"
          and active(page).locator('.pd-row[data-version-id="LatestMR"]').count() == 1)
    active(page).locator(".pd-card").scroll_into_view_if_needed()
    screenshot(page, "modrinth-fixed-version-history")
    active(page).get_by_role("button", name="1.20.1", exact=True).click()
    nested = expand_dependencies(page, "ExactMR1")
    nested_link = nested.get_by_role("button", name="打开前置 Nested dependency 的详情", exact=True)
    nested_link.focus()
    child_state = frame_state(page, True)
    before_nested = page.evaluate("window.__communityDependencies.saves.length")
    nested_link.click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Nested dependency")
    no_auto_actions(page, "二级前置也只导航，不产生安装或另存为动作", before_nested)
    back(page)
    compare_frame_state("从二级前置返回保持子详情 DOM、过滤、展开、滚动与触发焦点", child_state, frame_state(page))
    page.evaluate("window.__rememberedDetail = document.querySelectorAll('.resources-detail')[0]")
    back(page)
    compare_frame_state("从固定前置返回保持父详情 DOM、过滤、两组展开、前置展开、滚动与焦点", parent_state, frame_state(page))
    second_link = panel(page, "MainMRCurrent").get_by_role("button", name="打开前置 Pinned dependency 的详情", exact=True).filter(has_text="要求版本 1.0.0")
    second_link.click()
    expect(active(page).locator('.pd-row[data-required="true"]')).to_have_attribute("data-version-id", "ExactMR2")
    second_save = save_version(page, "ExactMR2")
    check("同一项目的另一个固定版本 ExactMR2 独立保留并按原 ID 另存为", second_save["args"]["versionId"] == "ExactMR2")
    back(page)
    panel(page, "MainMRCurrent").get_by_role("button", name="打开前置 Fabric API 的详情", exact=True).click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Fabric API")
    check("project-only API 不冒充固定 latest，详情明确要求手动选兼容文件", "未指定前置版本" in active(page).locator(".pd-dependency-context").inner_text()
          and active(page).locator('[data-required="true"]').count() == 0)
    active(page).get_by_role("button", name="打开项目页", exact=True).click()
    show_all_groups(page)
    expand_history(page)
    api_save = save_version(page, "FabricOld")
    check("project-only 前置允许手动选择历史文件并使用普通另存为入口", api_save["args"]["versionId"] == "FabricOld" and api_save["args"]["slugOrId"] == "FabricAPI")
    back(page)
    panel(page, "MainMRCurrent").get_by_role("button", name="打开前置 Quilt API 的详情", exact=True).click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Quilt API")
    check("Quilt API 保留实际 Quilt 加载器文件与普通版本下载", "Quilt" in active(page).inner_text() and active(page).locator('.pd-row[data-version-id="QuiltNew"]').count() == 1)
    back(page)
    back(page, False)
    restored_search = search_state(page)
    check("返回检索保持原 DOM、关键词、MC 条件、加载器、来源、第二页、结果和滚动", restored_search["sameDOM"]
          and all(saved_search[key] == restored_search[key] for key in ("query", "mcVersion", "loader", "source", "page", "results", "focus"))
          and abs(saved_search["top"] - restored_search["top"]) <= 2, {"before": saved_search, "after": restored_search})
    select_option(page, ".cv-toolbar__source .nw-select__trigger", "CurseForge")
    open_project(page, "CurseForge dependency regression")
    show_all_groups(page)
    expand_history(page)
    cf_block = expand_dependencies(page, "50000")
    check("CurseForge 仅 numeric relationType 3，保留两 API 和同名不同 ID", cf_block.locator(".pd-deps__item").count() == 5
          and cf_block.get_by_role("button", name="打开前置 Fabric API 的详情", exact=True).count() == 1
          and cf_block.get_by_role("button", name="打开前置 Quilt API 的详情", exact=True).count() == 1
          and cf_block.get_by_role("button", name="打开前置 Same named required project 的详情", exact=True).count() == 2)
    check("CurseForge 非必要及字符串 relationType 不触发解析", not page.evaluate("window.__communityDependencies.calls.some(c => c.command === 'community_dependency_resolve' && String(c.args.projectId).startsWith('990'))"))
    cf_cancel = save_version(page, "50000")
    check("CurseForge 失败前置不阻止主文件另存为取消", cf_cancel["taskId"] is None and cf_cancel["args"]["projectId"] == "10000" and cf_cancel["args"]["fileId"] == "50000")
    cf_save = save_version(page, "50000", "save")
    check("CurseForge 主资源保存也使用既有受管另存为通道", cf_save["taskId"] is not None and cf_save["args"]["fileId"] == "50000")
    cf_block.get_by_role("button", name="重试读取前置 项目 10103", exact=True).click()
    wait_ready(page, "50000")
    check("CurseForge 前置网络失败可重试恢复且不安装", cf_block.locator(".pd-deps__project").count() == 5 and cf_block.locator('[role="alert"]').count() == 0)
    cf_history = expand_dependencies(page, "50001")
    check("CurseForge 必要前置也按具体文件分组，不混入当前文件", cf_history.locator(".pd-deps__item").count() == 1
          and "Historical dependency only" in cf_history.inner_text() and "Historical dependency only" not in cf_block.inner_text())
    screenshot(page, "curseforge-required-dependencies")
    cf_link = cf_block.get_by_role("button", name="打开前置 Fabric API 的详情", exact=True)
    cf_link.focus()
    cf_parent = frame_state(page, True)
    before_cf_link = page.evaluate("window.__communityDependencies.saves.length")
    cf_link.click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Fabric API")
    no_auto_actions(page, "CurseForge 前置点击只进入自身平台详情", before_cf_link)
    check("CurseForge project-only 前置明确未指定版本且无 latest 指定标记", "未指定前置版本" in active(page).locator(".pd-dependency-context").inner_text()
          and active(page).locator('[data-required="true"]').count() == 0)
    show_all_groups(page)
    expand_history(page)
    cf_api_save = save_version(page, "60001")
    check("CurseForge 前置手动选旧文件时保持 projectId/fileId 身份并走另存为", cf_api_save["args"]["projectId"] == "306612" and cf_api_save["args"]["fileId"] == "60001")
    active(page).get_by_role("button", name="打开项目页", exact=True).click()
    back(page)
    compare_frame_state("CurseForge 返回保持父详情 DOM、筛选、展开与滚动焦点", cf_parent, frame_state(page))
    panel(page, "50000").get_by_role("button", name="打开前置 Quilt API 的详情", exact=True).click()
    expect(active(page).locator(".pd-card__title")).to_have_text("Quilt API")
    quilt_cf_save = save_version(page, "70001")
    check("CurseForge Quilt API 未过滤且下载仍按原文件编号", quilt_cf_save["args"]["projectId"] == "634179" and quilt_cf_save["args"]["fileId"] == "70001")
    back(page)
    all_calls = page.evaluate("window.__communityDependencies.calls")
    check("两源详情历史读取不带截断兼容条件且解析仅读取声明标识", all(call["args"]["gameVersion"] == "" and call["args"]["loader"] == ""
          for call in all_calls if call["command"] in ("modrinth_project_versions", "curseforge_project_files")))
    check("全流程未调用安装命令且没有未预期 IPC", not page.evaluate("window.__communityDependencies.unexpected.length")
          and not any(call["command"].endswith("_install") for call in all_calls))
    report["functionalIPC"] = page.evaluate("({calls: window.__communityDependencies.calls, saves: window.__communityDependencies.saves, attempts: window.__communityDependencies.attempts, taskCount: window.__communityDependencies.taskCount()})")


def metadata_failure(page):
    open_mod(page)
    for source, title, version_id in (("Modrinth", "Modrinth dependency regression", "MainMRCurrent"), ("CurseForge", "CurseForge dependency regression", "50000")):
        if source == "CurseForge":
            select_option(page, ".cv-toolbar__source .nw-select__trigger", source)
        open_project(page, title)
        block = expand_dependencies(page, version_id)
        check(f"{source} 旧响应缺少前置元数据时按精确版本读取且失败可重试", "Required metadata network failure" in block.inner_text()
              and block.get_by_role("button", name="重试前置检测", exact=True).count() == 1)
        cancelled = save_version(page, version_id)
        check(f"{source} 整体前置检测失败仍不阻碍主资源另存为", cancelled["taskId"] is None)
        block.get_by_role("button", name="重试前置检测", exact=True).click()
        wait_ready(page, version_id)
        check(f"{source} 整体检测重试只恢复该版本必要元数据", block.locator(".pd-deps__item").count() >= 5
              and block.get_by_role("button", name="重试前置检测", exact=True).count() == 0)
        back(page, False)
    calls = page.evaluate("window.__communityDependencies.calls")
    parent_lookups = [call["args"] for call in calls
                      if call["command"] == "community_dependency_resolve"
                      and call["args"].get("projectId") in ("ParentMR", "10000")]
    check("元数据回退从未猜测 latest 或调用安装", not page.evaluate("window.__communityDependencies.unexpected.length")
          and not any(call["command"].endswith("_install") for call in calls)
          and {(lookup["platform"], lookup["projectId"], lookup["versionId"]) for lookup in parent_lookups}
          == {("modrinth", "ParentMR", "MainMRCurrent"), ("curseforge", "10000", "50000")}, parent_lookups)
    report["metadataIPC"] = page.evaluate("({calls: window.__communityDependencies.calls, saves: window.__communityDependencies.saves, attempts: window.__communityDependencies.attempts})")


GEOMETRY = """input => {
  const field = input.closest('.mr-search');
  const rect = node => {
    const r = node.getBoundingClientRect();
    return {x:r.x,y:r.y,width:r.width,height:r.height,left:r.left,right:r.right,top:r.top,bottom:r.bottom};
  };
  const fieldRect = rect(field), inputRect = rect(input), clips = [], violations = [];
  for (let node = field.parentElement; node; node = node.parentElement) {
    const s = getComputedStyle(node), r = rect(node);
    const clipX = s.overflowX !== 'visible', clipY = s.overflowY !== 'visible';
    if (clipX || clipY) {
      const item = {className:node.className,overflowX:s.overflowX,overflowY:s.overflowY,rect:r};
      clips.push(item);
      if ((clipX && (fieldRect.left < r.left - .5 || fieldRect.right > r.right + .5))
        || (clipY && (fieldRect.top < r.top - .5 || fieldRect.bottom > r.bottom + .5))) violations.push(item);
    }
  }
  const style = getComputedStyle(input), fieldStyle = getComputedStyle(field);
  return {field:fieldRect,input:inputRect,clips,violations,focus:document.activeElement===input,
    selectionStart:input.selectionStart,selectionEnd:input.selectionEnd,value:input.value,
    shadow:fieldStyle.boxShadow,outline:fieldStyle.outline,outlineOffset:fieldStyle.outlineOffset,
    fontSize:style.fontSize,lineHeight:style.lineHeight,userSelect:style.userSelect,
    window:{width:innerWidth,height:innerHeight,dpr:devicePixelRatio},
    withinViewport:fieldRect.left >= 0 && fieldRect.top >= 0 && fieldRect.right <= innerWidth && fieldRect.bottom <= innerHeight,
    inputContained:inputRect.left >= fieldRect.left && inputRect.right <= fieldRect.right && inputRect.top >= fieldRect.top && inputRect.bottom <= fieldRect.bottom};
}"""

PIXELS = """async ({baseline, focused, selected, geometry}) => {
  const decode = async encoded => {
    const image = await createImageBitmap(await (await fetch('data:image/png;base64,' + encoded)).blob());
    const canvas = new OffscreenCanvas(image.width, image.height), context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    return {width:image.width,height:image.height,data:context.getImageData(0,0,image.width,image.height).data};
  };
  const [before, focus, selection] = await Promise.all([decode(baseline), decode(focused), decode(selected)]);
  const scale = geometry.window.dpr, r=geometry.field, input=geometry.input;
  const sample=(image,x,y)=>{
    const offset=(Math.max(0,Math.min(image.height-1,Math.floor(y*scale)))*image.width+Math.max(0,Math.min(image.width-1,Math.floor(x*scale))))*4;
    return [...image.data.slice(offset,offset+3)];
  };
  const distance=(a,b)=>Math.max(...a.map((v,i)=>Math.abs(v-b[i])));
  const edges={};
  for (const side of ['left','right','top','bottom']) {
    const readings=[];
    for(let i=0;i<15;i++) {
      const ratio=.25+.5*i/14;
      let x,y,insideX,insideY;
      if(side==='left'||side==='right') {
        x=side==='left'?r.left+.8:r.right-.8; y=r.top+r.height*ratio;
        insideX=side==='left'?r.left+7:r.right-7;insideY=y;
      } else {
        x=r.left+r.width*ratio;y=side==='top'?r.top+.8:r.bottom-.8;
        insideX=x;insideY=side==='top'?r.top+5:r.bottom-5;
      }
      const edge=sample(focus,x,y), baselineEdge=sample(before,x,y), inside=sample(focus,insideX,insideY);
      readings.push({edge,baselineEdge,inside,focusDelta:distance(edge,baselineEdge),ringContrast:distance(edge,inside)});
    }
    edges[side]={coverage:readings.filter(v=>v.focusDelta>3&&v.ringContrast>3).length/readings.length,readings};
  }
  let selectionPixels=0;
  for(let y=Math.ceil(input.top*scale);y<Math.floor(input.bottom*scale);y++) {
    for(let x=Math.ceil(input.left*scale);x<Math.floor(input.right*scale);x++) {
      const offset=(y*focus.width+x)*4;
      if(Math.max(...[0,1,2].map(i=>Math.abs(focus.data[offset+i]-selection.data[offset+i])))>4)selectionPixels++;
    }
  }
  return {dimensions:{width:focus.width,height:focus.height},edges,selectionPixels};
}"""


def focus_capture(browser, scale, width, height, name):
    viewport = {"width": math.floor(width / scale), "height": math.floor(height / scale)}
    context = browser.new_context(viewport=viewport, device_scale_factor=scale, reduced_motion="reduce")
    try:
        page = context.new_page()
        attach_session(page, name)
        navigate(page, args.url)
        open_mod(page)
        search = page.get_by_role("searchbox", name="搜索Mod")
        search.fill("Nickel Mod")
        page.wait_for_timeout(450)
        search.evaluate("input => input.blur()")
        baseline = screenshot(page, f"{name}-baseline")
        search.focus()
        search.press("End")
        page.wait_for_timeout(200)
        focused = screenshot(page, f"{name}-focus")
        search.press("Control+A")
        page.wait_for_timeout(100)
        geometry = search.evaluate(GEOMETRY)
        selected = screenshot(page, f"{name}-selection")
        pixels = page.evaluate(PIXELS, {"baseline": base64.b64encode(baseline).decode(), "focused": base64.b64encode(focused).decode(), "selected": base64.b64encode(selected).decode(), "geometry": geometry})
        case = {"name": name, "scale": scale, "physicalWindow": {"width": width, "height": height}, "layoutViewport": viewport, "geometry": geometry, "pixels": pixels}
        report["focusCases"].append(case)
        check(f"{name} 搜索焦点与全选可用且输入完整位于裁切边界内", geometry["focus"] and geometry["selectionStart"] == 0
              and geometry["selectionEnd"] == len(geometry["value"]) and geometry["withinViewport"] and geometry["inputContained"]
              and not geometry["violations"] and geometry["userSelect"] == "text"
              and geometry["input"]["height"] + .5 >= float(geometry["lineHeight"].replace("px", "")), geometry)
        check(f"{name} 四边焦点环像素均可见且全选具有实际像素证据", "inset" in geometry["shadow"]
              and all(edge["coverage"] >= .8 for edge in pixels["edges"].values()) and pixels["selectionPixels"] >= 20,
              {"coverage": {side: edge["coverage"] for side, edge in pixels["edges"].items()}, "selectionPixels": pixels["selectionPixels"]})
    finally:
        context.close()


with sync_playwright() as playwright:
    browser = None
    launch_errors = []
    for channel in ("msedge", "chrome", None):
        try:
            browser = playwright.chromium.launch(headless=True, **({"channel": channel} if channel else {}))
            report["browserChannel"] = channel or "cached-chromium"
            report["browserVersion"] = browser.version
            break
        except Exception as error:
            launch_errors.append(str(error))
    if browser is None:
        raise RuntimeError("Cannot launch a cached browser: " + "\n".join(launch_errors))
    try:
        if args.inspect:
            page = browser.new_page(viewport={"width": 1100, "height": 850}, reduced_motion="reduce")
            attach_session(page, "inspect")
            navigate(page, args.url)
            open_mod(page)
            screenshot(page, "inspect-search")
            open_project(page, "Modrinth dependency regression")
            expand_dependencies(page, "MainMRCurrent")
            screenshot(page, "inspect-dependencies")
            report["inspectDOM"] = page.locator("body").inner_text()
            report["inspectIPC"] = page.evaluate("window.__communityDependencies.calls")
            page.close()
        else:
            for name, scenario, run in (("functional", "normal", functional), ("metadata-failure", "metadata", metadata_failure)):
                context = browser.new_context(viewport={"width": 1100, "height": 850}, reduced_motion="reduce")
                page = context.new_page()
                session = attach_session(page, name)
                try:
                    navigate(page, args.url + f"?scenario={scenario}")
                    run(page)
                except Exception as error:
                    report["errors"].append({"session": name, "error": str(error), "body": page.locator("body").inner_text()})
                    session["ipcAtFailure"] = page.evaluate("window.__communityDependencies?.calls")
                    screenshot(page, f"{name}-failure")
                finally:
                    context.close()
            for scale in (1, 1.25, 1.5):
                for width, label in ((1100, "normal"), (760, "narrow")):
                    name = f"search-{label}-{round(scale*100)}"
                    try:
                        focus_capture(browser, scale, width, 850, name)
                    except Exception as error:
                        report["errors"].append({"session": name, "error": str(error)})
        for session in report["sessions"]:
            check(f"{session['name']} 无浏览器异常或非本地网络副作用", not session["consoleErrors"] and not session["pageErrors"] and not session["remoteRequests"], session)
    except Exception as error:
        report["errors"].append({"session": "runner", "error": str(error)})
    finally:
        browser.close()
        report["status"] = "fail" if report["errors"] or any(not item["ok"] for item in report["checks"]) else "pass"
        report["finishedAt"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (output / "browser-results.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"status": report["status"], "passedChecks": sum(item["ok"] for item in report["checks"]), "failedChecks": [item for item in report["checks"] if not item["ok"]], "errors": report["errors"], "evidence": str(output / "browser-results.json")}, ensure_ascii=False, indent=2))
        if report["status"] != "pass":
            raise SystemExit(1)
