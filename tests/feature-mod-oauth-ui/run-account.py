"""Account-only entry point: settings protection, scope and login notifications."""
import argparse
import json
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from oauth_checks import verify_oauth_home, verify_login_feedback
from navigation_checks import verify_account_return

parser = argparse.ArgumentParser()
parser.add_argument('--url', required=True)
args = parser.parse_args()
checks = []
evidence = Path('tests/feature-mod-oauth-ui/evidence')
evidence.mkdir(exist_ok=True)


def check(name, condition):
    checks.append({'name': name, 'ok': bool(condition)})
    assert condition, name
    print('PASS:', name)


with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page(viewport={'width': 1100, 'height': 780})
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(args.url + '/tests/feature-mod-oauth-ui/account-settings.html')
    page.wait_for_load_state('networkidle')
    page.get_by_role('tab', name='OAuth2 方式', exact=True).click()
    secret = page.get_by_label('Client Secret', exact=True)
    expect(secret).to_be_visible()
    secret.focus()
    expect(secret).to_have_attribute('type', 'password')
    expect(secret).to_have_value('fixture-secret-long-value')
    check('Secret remains masked on focus', True)
    secret.fill('edited-alpha-secret')
    page.wait_for_timeout(400)
    page.evaluate("window.__pill = document.querySelector('.skin-auth-settings > .ni-seg'); window.__pillStyle = window.__pill.querySelector('.ni-seg__pill').getAttribute('style')")
    page.get_by_role('button', name='Switch site', exact=True).click()
    expect(page.get_by_text('Beta Skin', exact=True)).to_be_visible()
    expect(secret).to_have_value('fixture-secret-long-value')
    page.get_by_role('button', name='Switch site', exact=True).click()
    expect(secret).to_have_value('edited-alpha-secret')
    check('custom credentials stay scoped to their site', True)
    check('server edits keep the auth selector and its pill mounted', page.evaluate("window.__pill === document.querySelector('.skin-auth-settings > .ni-seg') && window.__pillStyle === window.__pill.querySelector('.ni-seg__pill').getAttribute('style')"))
    callback = page.get_by_label('回调 URI', exact=True)
    expect(callback).to_be_disabled()
    expect(callback).to_have_attribute('readonly', '')
    expect(callback).to_have_value('nickel://auth')
    expect(page.get_by_role('button', name='恢复预设', exact=True)).to_have_count(0)
    check('callback is fixed and reset preset is absent', True)
    for theme in ['light', 'dark']:
        page.evaluate('(theme)=>document.documentElement.dataset.theme=theme', theme)
        client = page.get_by_label('Client ID', exact=True)
        client.focus()
        page.wait_for_timeout(350)
        check(f'{theme}: focus ring is not clipped', client.evaluate('''n=>{
          const a=n.getBoundingClientRect(), b=n.closest('.ni-collapse').getBoundingClientRect();
          return a.left-b.left>=3 && b.right-a.right>=3 && a.top-b.top>=3 && b.bottom-a.bottom>=3;
        }'''))
        check(f'{theme}: callback is visually muted', callback.evaluate('n=>getComputedStyle(n).color') != client.evaluate('n=>getComputedStyle(n).color'))
    check('auth pill fits content', page.locator('.skin-auth-settings > .ni-seg').evaluate('''n=>{
      const items=[...n.querySelectorAll('.ni-seg__item')];
      return Math.abs(n.getBoundingClientRect().width-items.reduce((w,i)=>w+i.getBoundingClientRect().width,0)-9)<2;
    }'''))
    for server in ['https://littleskin.cn', 'https://LITTLESKIN.CN:443/api/yggdrasil/', 'littleskin.cn', '   ']:
        page.evaluate('(server)=>window.__feature.setServer(server)', server)
        expect(secret).to_have_count(0)
        expect(page.get_by_text('自定义 OAuth', exact=True)).to_have_count(0)
    check('built-in credentials are never requested', page.evaluate("window.__feature.calls.filter(c=>c.command==='skin_oauth_config_get' && window.__feature.isBuiltinSkinSite(c.args.server)).length") == 0)
    page.evaluate("window.__feature.setScope({root:'C:/OAuthFixture',versionId:'v'}); window.__feature.setServer('https://a.example')")
    expect(secret).to_have_value('fixture-secret-long-value')
    check('instance does not reuse global client draft', True)
    page.evaluate('window.__feature.setScope({})')
    expect(secret).to_have_value('edited-alpha-secret')
    check('lookalike hosts and HTTP cannot access built-in credentials', page.evaluate("['https://littleskin.cn.example','http://littleskin.cn','https://littleskin.cn/other','https://littleskin.cn:8443'].every(s=>!window.__feature.isBuiltinSkinSite(s))"))
    page.get_by_role('tab', name='经典登录方式', exact=True).click()
    expect(secret).not_to_be_visible()
    check('classic mode hides custom OAuth fields', True)

    page.route('https://**/favicon.png', lambda route: route.fulfill(status=200, content_type='image/svg+xml', body='<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#39a"/></svg>'))
    # Match the existing login suite: avoid clicking a Home form during its route swap.
    # Toast motion itself is exercised with real frames by tests/toast-ui/run.py.
    page.emulate_media(reduced_motion='reduce')
    page.goto(args.url + '/tests/feature-mod-oauth-ui/home.html')
    page.wait_for_load_state('networkidle')
    expect(page.locator('.home__name')).to_have_text('Original Microsoft')
    page.evaluate("window.__home.select('a')")
    expect(page.locator('.home__name')).to_have_text('Alpha Player')
    check('exclusive mode selects the same site and hides tabs', page.get_by_role('tab').count() == 0)
    page.evaluate("window.__home.select('b')")
    expect(page.locator('.home__name')).to_have_text('Beta Player')
    page.evaluate("window.__home.select('normal')")
    expect(page.locator('.home__name')).to_have_text('Original Microsoft')
    check('leaving exclusive mode restores ordinary account', page.evaluate('window.__home.active()') == 'msa')
    page.evaluate("window.__home.select('empty')")
    button = page.get_by_role('button', name='使用 Beta Skin 登录', exact=True)
    expect(button).to_be_enabled()
    button.click()
    page.get_by_role('button', name='取消', exact=True).click()
    expect(page.get_by_role('dialog')).to_have_count(0)
    page.evaluate('window.__home.releaseBegin()')
    page.wait_for_timeout(250)
    check('cancelled OAuth preparation never starts finish', not page.evaluate("window.__home.calls.some(c=>c.command==='skin_oauth_finish')"))
    verify_oauth_home(page, check, args.url)
    verify_login_feedback(page, check, args.url)
    verify_account_return(page, check, args.url)
    check('account regression has no runtime errors', not errors)
    browser.close()
(evidence / 'account-report.json').write_text(json.dumps(checks, ensure_ascii=False, indent=2), encoding='utf-8')
print(f'{len(checks)} account checks passed')
