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
    return active(page).locator('.pd-group').filter(has=page.locator(f'.pd-row[data-version-id="{version_id}"]')).locator('.pd-deps')


def wait_ready(page, version_id):
    expect(panel(page, version_id)).to_have_attribute('aria-busy', 'false')


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
    # Scroll each group so lazy version rows can identify their owning panel.
    show_all_groups(page)
    for group in active(page).locator('.pd-group').all():
        toggle = group.locator('.pd-group__toggle')
        if toggle.get_attribute('aria-expanded') != 'true':
            toggle.click()
        group.locator('.pd-list').scroll_into_view_if_needed()
        page.wait_for_timeout(60)
    wait_ready(page, version_id)
    return panel(page, version_id)


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
        dependencies: [...node.querySelectorAll('.pd-deps__project')].map(button => button.getAttribute('aria-label')),
        top: document.querySelector('.ni-content').scrollTop,
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
        loader: host.querySelector('.cv-loader .ni-select__trigger').textContent,
        source: host.querySelector('.cv-toolbar__source .ni-select__trigger').textContent,
        page: host.querySelector('.cv-pager__page')?.textContent,
        top: host.querySelector('.ni-pager__page').scrollTop,
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
    check('两平台同 slug 仍为独立项目', page.get_by_role('button', name='查看 Modrinth dependency regression 的版本', exact=True).count() == 1
          and page.get_by_role('button', name='查看 CurseForge dependency regression 的版本', exact=True).count() == 1)
    search = page.get_by_role('searchbox', name='搜索Mod')
    search.fill('Nickel retained search')
    mc = page.get_by_role('combobox', name='MC 版本', exact=True)
    mc.fill('1.20.1'); mc.press('Enter')
    select_option(page, '.cv-loader .ni-select__trigger', 'NeoForge')
    select_option(page, '.cv-toolbar__source .ni-select__trigger', 'Modrinth')
    page.wait_for_function("window.__communityDependencies.calls.filter(c=>c.command==='modrinth_search').at(-1)?.args.query==='Nickel retained search'")
    page.get_by_role('button', name='下一页', exact=True).click()
    expect(page.locator('.cv-pager__page')).to_have_text('2')
    page.wait_for_timeout(450)
    card = page.get_by_role('button', name='查看 Modrinth dependency regression 的版本', exact=True)
    card.focus()
    page.locator('.ni-pager__page').evaluate('node=>node.scrollTop=160')
    saved_search = search_state(page, True)
    check('返回检索测试确实具有第二页和非零滚动', saved_search['top'] > 0 and saved_search['page'] == '2')
    card.click()
    block = expand_dependencies(page, 'MainMRCurrent')
    check('进入详情即加载，无逐文件前置折叠入口', active(page).locator('.pd-deps__toggle').count() == 0
          and page.evaluate("window.__communityDependencies.calls.some(c=>c.command==='community_dependency_resolve')"))
    check('加载器 API 排除、同名项目分离、同项目多固定版本合并展示', block.locator('.pd-deps__item').count() == 8
          and block.get_by_role('button', name='打开前置 Same named required project 的详情', exact=True).count() == 2
          and block.get_by_role('button', name='打开前置 Pinned dependency 的详情', exact=True).count() == 1
          and not any(text in block.inner_text() for text in ['Fabric API', 'Quilt API', 'OptionalMR', 'IncompatibleMR', 'EmbeddedMR', 'Historical dependency only']))
    check('仅文件名关系保留可执行错误，不猜测项目', block.locator('.pd-deps__unresolved').filter(has_text='manual-required.jar').count() == 1
          and block.locator('.pd-deps__unresolved').filter(has_text='manual-quilt.jar').count() == 1)
    check('错误版本与项目身份返回均被拒绝', '返回的前置版本与声明不一致' in block.inner_text()
          and '指定前置版本不属于返回的项目' in block.inner_text())
    tasks_before = page.evaluate('window.__communityDependencies.taskCount()')
    cancelled = save_version(page, 'MainMRCurrent')
    check('依赖失败不阻止取消另存为且不建任务', cancelled['taskId'] is None and page.evaluate('window.__communityDependencies.taskCount()') == tasks_before)
    saved = save_version(page, 'MainMRCurrent', 'save')
    check('主文件仍经正常另存为通道', saved['taskId'] is not None and saved['args']['slugOrId'] == 'ParentMR'
          and saved['args']['instanceVersionId'] == 'fixture-instance' and saved['args']['dir'] == 'C:/NickelDependencyFixture')
    before_retry = page.evaluate('({...window.__communityDependencies.attempts})')
    block.get_by_role('button', name='重试', exact=True).first.click()
    expect(block.get_by_role('button', name='打开前置 Retry dependency 的详情', exact=True)).to_be_visible()
    wait_ready(page, 'MainMRCurrent')
    after_retry = page.evaluate('window.__communityDependencies.attempts')
    check('重试只请求失败项，成功结果和版本关系复用', all(after_retry[key] == value for key,value in before_retry.items()
          if not any(failed in key for failed in ['UnstableMR', 'WrongPinnedMR', 'IdentityMR']))
          and block.locator('.pd-deps__project').count() == 6 and block.get_by_role('button', name='重试', exact=True).count() == 0)
    history = expand_dependencies(page, 'MainMRHistory')
    check('历史组保持自身关系，不混入当前文件', history.locator('.pd-deps__item').count() == 1
          and 'Historical dependency only' in history.inner_text() and 'Historical dependency only' not in block.inner_text())
    calls_before = page.evaluate("window.__communityDependencies.calls.filter(c=>c.command==='community_dependency_resolve').length")
    toggle = active(page).locator('.pd-group__toggle').first
    toggle.click(); toggle.click()
    page.wait_for_timeout(250)
    check('外层重新展开不重复请求', calls_before == page.evaluate("window.__communityDependencies.calls.filter(c=>c.command==='community_dependency_resolve').length"))
    screenshot(page, 'modrinth-grouped-prerequisites')

    exact_link = block.get_by_role('button', name='打开前置 Strict pinned dependency 的详情', exact=True)
    exact_link.focus()
    parent_state = frame_state(page, True)
    saves_before = page.evaluate('window.__communityDependencies.saves.length')
    exact_link.click()
    expect(active(page).locator('.pd-row[data-required="true"]')).to_have_attribute('data-version-id', 'WrongExact1')
    no_auto_actions(page, '唯一指定版本导航不产生下载或安装', saves_before)
    exact_save = save_version(page, 'WrongExact1')
    check('唯一指定文件另存为保留原 ID', exact_save['args']['versionId'] == 'WrongExact1' and exact_save['args']['slugOrId'] == 'WrongPinnedMR')
    back(page)
    compare_frame_state('返回父页恢复 DOM、筛选、展开、滚动和焦点', parent_state, frame_state(page))

    ambiguous = panel(page, 'MainMRCurrent').get_by_role('button', name='打开前置 Pinned dependency 的详情', exact=True)
    ambiguous.focus()
    parent_state = frame_state(page, True)
    ambiguous.click()
    expect(active(page).locator('.pd-error')).to_be_visible()
    check('多指定版本不擅自选择单一版本，历史失败仍真实显示', active(page).locator('[data-required="true"]').count() == 0
          and 'History metadata network failure' in active(page).inner_text()
          and active(page).locator('.pd-dependency-context').count() == 0)
    active(page).locator('.pd-error').get_by_role('button', name='重试', exact=True).click()
    expect(active(page).locator('.pd-error')).to_have_count(0)
    show_all_groups(page)
    for toggle in active(page).locator('.pd-group__toggle').all():
        if toggle.get_attribute('aria-expanded') != 'true': toggle.click()
    check('多固定版本的文件历史完整保留且不标记伪指定', active(page).locator('[data-required="true"]').count() == 0)
    for version_id in ['ExactMR1', 'ExactMR2', 'LatestMR']:
        for group in active(page).locator('.pd-group').all(): group.scroll_into_view_if_needed()
        result = save_version(page, version_id)
        check(f'{version_id} 手动下载保持文件身份', result['args']['versionId'] == version_id and result['args']['slugOrId'] == 'PinnedMR')
    nested = expand_dependencies(page, 'ExactMR1')
    nested_link = nested.get_by_role('button', name='打开前置 Nested dependency 的详情', exact=True)
    nested_link.focus()
    child_state = frame_state(page, True)
    nested_link.click()
    expect(active(page).locator('.pd-card__title')).to_have_text('Nested dependency')
    expect(active(page).locator('.pd-row')).to_be_visible()
    check('无依赖详情不显示空前置区或零数量', active(page).locator('.pd-deps').count() == 0
          and '0 项前置' not in active(page).inner_text() and '无必要前置' not in active(page).inner_text())
    back(page)
    compare_frame_state('二级详情返回保持子页状态', child_state, frame_state(page))
    page.evaluate("window.__rememberedDetail=document.querySelectorAll('.resources-detail')[0]")
    back(page)
    compare_frame_state('多层返回保持父页状态', parent_state, frame_state(page))
    check('返回父页不重复请求父项目依赖', after_retry == {key:value for key,value in page.evaluate('window.__communityDependencies.attempts').items() if key in after_retry})
    back(page, False)
    restored = search_state(page)
    check('返回检索保持条件、第二页、结果和滚动', restored['sameDOM']
          and all(saved_search[key] == restored[key] for key in ['query','mcVersion','loader','source','page','results','focus'])
          and abs(saved_search['top']-restored['top']) <= 2, {'before':saved_search,'after':restored})

    select_option(page, '.cv-toolbar__source .ni-select__trigger', 'CurseForge')
    open_project(page, 'CurseForge dependency regression')
    cf_block = expand_dependencies(page, '50000')
    check('CurseForge 只展示 required 且排除两 API', cf_block.locator('.pd-deps__item').count() == 3
          and cf_block.get_by_role('button', name='打开前置 Same named required project 的详情', exact=True).count() == 2
          and not any(text in cf_block.inner_text() for text in ['Fabric API','Quilt API']))
    check('CurseForge 非必要或字符串关系不请求', not page.evaluate("window.__communityDependencies.calls.some(c=>c.command==='community_dependency_resolve' && String(c.args.projectId).startsWith('990'))"))
    cf_cancel = save_version(page, '50000')
    check('CurseForge 依赖失败仍可取消另存为', cf_cancel['taskId'] is None and cf_cancel['args']['fileId'] == '50000')
    cf_save = save_version(page, '50000', 'save')
    check('CurseForge 主文件保存不走安装', cf_save['taskId'] is not None and cf_save['args']['projectId'] == '10000')
    cf_block.get_by_role('button', name='重试', exact=True).click()
    expect(cf_block.get_by_role('button', name='打开前置 Retry dependency 的详情', exact=True)).to_be_visible()
    cf_history = expand_dependencies(page, '50001')
    check('CurseForge 历史关系隔离', cf_history.locator('.pd-deps__item').count() == 1
          and 'Historical dependency only' in cf_history.inner_text() and 'Historical dependency only' not in cf_block.inner_text())
    cf_link = cf_block.get_by_role('button', name='打开前置 Same named required project 的详情', exact=True).first
    cf_link.focus(); cf_parent = frame_state(page, True); cf_link.click()
    expect(active(page).locator('.pd-row')).to_be_visible()
    check('project-only 前置无伪指定和解释装饰', active(page).locator('[data-required="true"]').count() == 0
          and active(page).locator('.pd-dependency-context').count() == 0)
    cf_result = save_version(page, '10101Current')
    check('CurseForge 前置手动下载保留平台与文件编号', cf_result['args']['projectId'] == '10101' and cf_result['args']['fileId'] == '10101Current')
    active(page).get_by_role('button', name='打开项目页', exact=True).click()
    back(page)
    compare_frame_state('CurseForge 返回保持父页状态', cf_parent, frame_state(page))
    all_calls = page.evaluate('window.__communityDependencies.calls')
    check('两源读取完整历史且无安装和未知 IPC', not page.evaluate('window.__communityDependencies.unexpected.length')
          and all(call['args']['gameVersion'] == '' and call['args']['loader'] == '' for call in all_calls if call['command'] in ['modrinth_project_versions','curseforge_project_files'])
          and not any(call['command'].endswith('_install') for call in all_calls))
    report['functionalIPC'] = page.evaluate('({calls:window.__communityDependencies.calls,saves:window.__communityDependencies.saves,attempts:window.__communityDependencies.attempts})')


def metadata_failure(page):
    open_mod(page)
    for source,title,version_id in [('Modrinth','Modrinth dependency regression','MainMRCurrent'),('CurseForge','CurseForge dependency regression','50000')]:
        if source == 'CurseForge': select_option(page,'.cv-toolbar__source .ni-select__trigger',source)
        open_project(page,title)
        block = expand_dependencies(page,version_id)
        check(f'{source} 旧响应缺少元数据按精确版本读取', 'Required metadata network failure' in block.inner_text())
        cancelled = save_version(page,version_id)
        check(f'{source} 元数据缺失不阻止主文件另存为', cancelled['taskId'] is None)
        block.get_by_role('button',name='重试',exact=True).first.click()
        expect(block.get_by_role('button',name='打开前置 Same named required project 的详情',exact=True)).to_have_count(2)
        check(f'{source} 精确元数据恢复后按分组显示且 API 排除', 'Required metadata network failure' not in block.inner_text()
              and 'Fabric API' not in block.inner_text() and 'Quilt API' not in block.inner_text())
        back(page,False)
    calls=page.evaluate('window.__communityDependencies.calls')
    lookups=[call['args'] for call in calls if call['command']=='community_dependency_resolve' and call['args'].get('projectId') in ['ParentMR','10000']]
    check('旧响应回退只请求原文件，不猜 latest', {(v['platform'],v['projectId'],v['versionId']) for v in lookups}
          == {('modrinth','ParentMR','MainMRCurrent'),('curseforge','10000','50000')})
    report['metadataIPC']=page.evaluate('({calls:window.__communityDependencies.calls,attempts:window.__communityDependencies.attempts})')


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
    for channel in ("msedge", "chrome"):
        try:
            browser = playwright.chromium.launch(headless=True, channel=channel)
            report["browserChannel"] = channel
            report["browserVersion"] = browser.version
            break
        except Exception as error:
            launch_errors.append(str(error))
    if browser is None:
        raise RuntimeError("Cannot launch an installed browser: " + "\n".join(launch_errors))
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
