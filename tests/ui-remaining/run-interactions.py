"""Pointer tooltips, Java option help, bare loading layout and unbound Save As."""
import argparse
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

parser = argparse.ArgumentParser()
parser.add_argument('--url', required=True)
args = parser.parse_args()
checks = []
evidence = Path('.verify/interactions')
evidence.mkdir(parents=True, exist_ok=True)


def check(label, value):
    assert value, label
    checks.append(label)
    print('PASS:', label)


def navigate(page, path):
    page.goto(args.url + path)
    page.wait_for_load_state('networkidle')


def follows(page, locator, text):
    locator.scroll_into_view_if_needed()
    box = locator.bounding_box()
    page.mouse.move(box['x'] + 10, box['y'] + box['height'] / 2)
    tip = page.get_by_role('tooltip')
    expect(tip).to_have_count(1)
    expect(tip).to_contain_text(text)
    expect(tip).to_have_attribute('data-side', 'pointer')
    page.wait_for_timeout(200)
    before = tip.bounding_box()
    page.mouse.move(box['x'] + 30, box['y'] + box['height'] / 2)
    page.wait_for_timeout(100)
    after = tip.bounding_box()
    check('pointer follows: ' + text, abs(after['x'] - before['x'] - 20) < 2)


def verify_tooltips(page):
    navigate(page, '/tests/ui-remaining/interactions.html')
    follows(page, page.locator('#input'), 'Input help')
    page.locator('#input').click()
    expect(page.get_by_role('tooltip')).to_have_attribute('data-side', 'pointer')
    check('mouse focus cannot replace pointer position', True)
    follows(page, page.locator('#parent'), 'Parent help')
    follows(page, page.locator('#nested'), 'Child help')
    page.locator('#nested').click()
    expect(page.get_by_role('tooltip')).to_have_count(1)
    expect(page.get_by_role('tooltip')).to_have_attribute('data-side', 'pointer')
    check('nested input has no native duplicate title', page.locator('#nested').get_attribute('title') is None)
    follows(page, page.get_by_role('button', name='Icon help'), 'Icon help')
    follows(page, page.get_by_role('button', name='Disabled control'), 'Disabled help')
    check('disabled control keeps normal arrow cursor', page.get_by_role('button', name='Disabled control').evaluate('n=>getComputedStyle(n).cursor') == 'default')
    page.mouse.move(1000, 750)
    page.locator('#nested').focus()
    page.keyboard.press('Tab')
    expect(page.locator('#input')).to_be_focused()
    expect(page.get_by_role('tooltip')).to_have_text('Input help')
    check('keyboard-only focus remains accessible', page.locator('#input').get_attribute('aria-describedby') is not None)
    page.keyboard.press('Escape')
    expect(page.get_by_role('tooltip')).to_have_count(0)


def verify_java(page):
    for scenario in ['global', 'instance']:
        navigate(page, '/tests/ui-remaining/interactions.html?scenario=' + scenario)
        trigger = page.locator('.ni-select__trigger').filter(has_text='自动选择').first
        expect(trigger).to_be_visible()
        follows(page, trigger, '启动游戏时')
        trigger.click()
        expect(page.get_by_role('tooltip')).to_have_count(0)
        follows(page, page.get_by_role('option', name='自动选择', exact=True), '启动游戏时')
        option = page.get_by_role('option').filter(has_text='Fixture Vendor')
        follows(page, option, 'C:/Fixture/Java21/bin/javaw.exe')
        option.click()
        trigger = page.locator('.ni-select__trigger').filter(has_text='Fixture Vendor')
        follows(page, trigger, 'C:/Fixture/Java21/bin/javaw.exe')
        page.mouse.move(1000, 750)
        expect(page.get_by_role('tooltip')).to_have_count(0)
        check(scenario + ' Java row does not show generic auto-selection help', True)


def verify_loading(page):
    for theme in ['light', 'dark']:
        for width, height in [(900, 700), (1200, 850)]:
            page.set_viewport_size({'width': width, 'height': height})
            navigate(page, f'/tests/loading-animation/regression.html?scenario=detail&theme={theme}')
            expect(page.locator('.ni-loading--detail')).to_be_visible()
            page.wait_for_timeout(400)
            data = page.locator('.ni-loading--detail').evaluate('''n=>{
              const r=n.getBoundingClientRect(), c=n.querySelector('.ni-loading__card').getBoundingClientRect();
              const s=getComputedStyle(n.querySelector('.ni-loading__card'));
              return { centered: Math.abs(r.x+r.width/2-c.x-c.width/2)<1 && Math.abs(r.y+r.height/2-c.y-c.height/2)<1,
                width: n.querySelector('.ni-loading__art').getBoundingClientRect().width,
                room: n.parentElement.getBoundingClientRect().height,
                background: s.backgroundColor, shadow:s.boxShadow,
                overflow: document.querySelector('.ni-content').scrollHeight > document.querySelector('.ni-content').clientHeight+2 };
            }''')
            check(f'{theme} {width}: bare loader centered in remaining content', data['centered'] and data['room'] > 250 and not data['overflow'])
            check(f'{theme} {width}: bare artwork enlarged 5%, without background', abs(data['width'] - 112*1.05) < 1 and data['background'] == 'rgba(0, 0, 0, 0)' and data['shadow'] == 'none')
            page.screenshot(path=str(evidence / f'loading-{theme}-{width}.png'))
    page.set_viewport_size({'width': 1100, 'height': 780})


def verify_save_as(page):
    for source, title in [('modrinth', 'Modrinth dependency regression'), ('curseforge', 'CurseForge dependency regression')]:
        navigate(page, '/tests/community-dependencies/regression.html')
        page.evaluate('''() => {
          window.__communityDependencies.setInstance(null);
          window.__root = 'D:/Fixture/SelectedRoot';
          const invoke = window.__TAURI_INTERNALS__.invoke;
          window.__TAURI_INTERNALS__.invoke = async (cmd,args) => {
            if (cmd === 'instances_get') {
              if (window.__pauseRoot) await new Promise(r=>window.__releaseRoot=r);
              return JSON.stringify({roots:[window.__root],active:window.__root});
            }
            if (cmd === 'resource_instance_context' && window.__pauseContext) {
              await new Promise(r=>window.__releaseContext=r);
            }
            return invoke(cmd,args);
          };
        }''')
        page.get_by_role('button', name='Mod', exact=True).click()
        project = page.get_by_role('button', name=f'查看 {title} 的版本', exact=True).first
        expect(project).to_be_visible()
        project.click()
        detail = page.locator('.resources-detail:not([hidden])')
        expect(detail.locator('.pd-group').first).to_be_visible()
        expect(detail.locator('.pd-card__empty')).to_have_count(0)
        toggle = detail.locator('.pd-group__toggle').first
        if toggle.get_attribute('aria-expanded') != 'true':
            toggle.click()
        detail.locator('.pd-group').first.locator('.pd-list').scroll_into_view_if_needed()
        row = detail.locator('.pd-row').first
        expect(row).to_be_enabled()

        def save():
            count = page.evaluate('window.__communityDependencies.saves.length')
            first_group = detail.locator('.pd-group').first
            if first_group.locator('.pd-group__toggle').get_attribute('aria-expanded') != 'true':
                first_group.locator('.pd-group__toggle').click()
            first_group.locator('.pd-list').scroll_into_view_if_needed()
            row.click()
            page.wait_for_function('n=>window.__communityDependencies.saves.length>n', arg=count)
            expect(row).to_be_enabled()
            return page.evaluate('window.__communityDependencies.saves.at(-1)')

        first = save()
        check(source + ' no-instance Save As defaults to selected root', first['args']['root'] == 'D:/Fixture/SelectedRoot' and first['args']['dir'] == 'D:/Fixture/SelectedRoot' and first['args']['instanceVersionId'] == '')
        check(source + ' cancelled dialog creates no task', first['taskId'] is None and page.evaluate('window.__communityDependencies.taskCount()') == 0)
        page.evaluate("window.__root='E:/Fixture/AnotherRoot'")
        second = save()
        check(source + ' Save As resolves root at click time', second['args']['root'] == 'E:/Fixture/AnotherRoot')

        page.evaluate('window.__pauseRoot=true; window.__releaseRoot=null')
        row.click()
        page.wait_for_function('typeof window.__releaseRoot === "function"')
        page.evaluate("window.__communityDependencies.setInstance({root:'C:/NickelDependencyFixture',versionId:'fixture-instance',loader:'fabric',icon:'fabric'}); window.__releaseRoot(); window.__pauseRoot=false")
        expect(detail.locator('.pd-error')).to_contain_text('当前实例已变化')
        check(source + ' no-instance resolution rejects an intervening selection', page.evaluate('window.__communityDependencies.saves.length') == 2)
        page.wait_for_timeout(300)
        selected = save()
        check(source + ' selected instance retains its resource root and ID', selected['args']['root'] == 'C:/NickelDependencyFixture' and selected['args']['instanceVersionId'] == 'fixture-instance')
        page.evaluate('window.__pauseContext=true; window.__releaseContext=null')
        row.click()
        page.wait_for_function('typeof window.__releaseContext === "function"')
        page.evaluate("window.__communityDependencies.setInstance({root:'C:/NickelDependencyFixture',versionId:'other-instance',loader:'fabric',icon:'fabric'}); window.__releaseContext(); window.__pauseContext=false")
        expect(detail.locator('.pd-error')).to_contain_text('当前实例已变化')
        check(source + ' instance revalidation rejects a late stale result', page.evaluate('window.__communityDependencies.saves.length') == 3)


with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page(viewport={'width': 1100, 'height': 780})
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    try:
        verify_tooltips(page)
        verify_java(page)
        verify_loading(page)
        verify_save_as(page)
        check('interaction regression has no runtime errors', not errors)
    finally:
        page.screenshot(path=str(evidence / 'last-state.png'))
        browser.close()
print(f'{len(checks)} interaction checks passed')
