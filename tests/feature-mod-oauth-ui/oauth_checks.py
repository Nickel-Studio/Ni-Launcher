"""OAuth button geometry, provider icons and result notifications (mocked IPC)."""
from playwright.sync_api import expect


def clear_feedback(page):
    page.evaluate('''() => {
      for (let i=1; i<=window.__home.lastToastId; i++) window.__home.dismissToast(i);
      window.__home.notifications = [];
      window.__home.startToastId = window.__home.lastToastId;
    }''')
    expect(page.locator('.ni-toast')).to_have_count(0)


def assert_feedback(page, check, label, tone, message):
    toast = page.locator('.ni-toast')
    expect(toast).to_have_count(1)
    expect(toast).to_have_attribute('data-tone', tone)
    expect(toast).to_contain_text(message)
    expect(page.locator('.account-form__error')).to_have_count(0)
    page.wait_for_function('window.__home.notifications.length > 0')
    check(label, page.evaluate('window.__home.notifications.length === 1 && window.__home.lastToastId === window.__home.startToastId + 1'))


def verify_oauth_home(page, check, base_url):
    button = page.get_by_role('button', name='使用 Beta Skin 登录', exact=True)
    expect(button).to_be_enabled()
    icon = button.locator(':scope > img')
    expect(icon).to_have_attribute('src', 'https://empty.example/favicon.png')
    page.wait_for_function("document.querySelector('.skin-oauth-login__button img').naturalWidth > 0")
    check('provider favicon is a separate leading icon', button.locator('span img, span svg').count() == 0)
    for width in [900, 1100, 1440]:
        page.set_viewport_size({'width': width, 'height': 780})
        page.wait_for_timeout(250)
        check(f'OAuth button stays compact and aligned at {width}', button.evaluate('''n=>{
          const b=n.getBoundingClientRect(), row=n.parentElement.getBoundingClientRect();
          const i=n.querySelector('.skin-site-icon').getBoundingClientRect(), t=n.lastElementChild.getBoundingClientRect();
          const ref=getComputedStyle(document.querySelector('#microsoft-reference .ni-btn')), s=getComputedStyle(n);
          return b.width<row.width-20 && Math.abs(b.x+b.width/2-row.x-row.width/2)<1
            && Math.abs(i.y+i.height/2-t.y-t.height/2)<2
            && ['height','fontSize','borderRadius','backgroundColor','gap'].every(k=>s[k]===ref[k]);
        }'''))
    page.set_viewport_size({'width': 1100, 'height': 780})
    page.locator('.home__card').screenshot(path='tests/feature-mod-oauth-ui/evidence/oauth-button.png')
    page.evaluate("document.querySelector('.skin-oauth-login__button img').dispatchEvent(new Event('error'))")
    expect(button.locator('.skin-site-icon--fallback')).to_have_text('B')
    check('favicon failure uses provider initial without broken image', button.locator('img,svg').count() == 0)

    def start():
        clear_feedback(page)
        page.evaluate("window.__home.oauthMode='finish'; window.__home.finish=null; window.__home.rejectFinish=null")
        button.click()
        expect(page.get_by_role('dialog')).to_be_visible()
        page.wait_for_function('typeof window.__home.finish === "function"')

    start()
    page.evaluate("window.__home.rejectFinish('你已在网页中取消授权')")
    expect(page.get_by_role('dialog')).to_have_count(0)
    assert_feedback(page, check, 'web denial only emits one cancellation toast without inline error', 'info', 'Beta Skin 登录已取消')
    expect(button).to_be_enabled()
    page.wait_for_timeout(250)
    toast = page.locator('.ni-toast')
    check('web denial notifies bottom left', toast.bounding_box()['x'] == 0 and toast.bounding_box()['y'] > 600)
    page.screenshot(path='tests/feature-mod-oauth-ui/evidence/oauth-denied.png')

    start()
    page.evaluate("window.__home.rejectFinish('授权已超时，请重新登录')")
    expect(page.get_by_role('dialog')).to_have_count(0)
    assert_feedback(page, check, 'OAuth timeout is an error rather than cancellation', 'error', '授权已超时')

    clear_feedback(page)
    page.evaluate("window.__home.oauthMode='begin-error'")
    button.click()
    expect(page.get_by_role('dialog')).to_have_count(0)
    assert_feedback(page, check, 'OAuth begin failure only emits error toast', 'error', '无法打开授权页面')

    for result in ['success', 'failure']:
        start()
        page.get_by_role('button', name='取消', exact=True).click()
        if result == 'success':
            page.evaluate("window.__home.finish({status:'account',account:{id:'late',name:'Late',server:'https://empty.example'}})")
        else:
            page.evaluate("window.__home.rejectFinish('你已在网页中取消授权')")
        expect(page.get_by_role('dialog')).to_have_count(0)
        page.wait_for_timeout(150)
        assert_feedback(page, check, f'local OAuth cancel ignores late {result}', 'info', '登录已取消')
        check(f'late OAuth {result} cannot activate account', page.evaluate('window.__home.active()') == '')

    start()
    page.evaluate("window.__home.finish({status:'select',sessionId:'oauth_cancel_multi',profiles:[{id:'4'.repeat(32),name:'First'}]})")
    expect(page.get_by_role('dialog', name='选择角色')).to_be_visible()
    page.evaluate('window.__home.pauseSelect=true')
    page.get_by_role('button', name='确认', exact=True).click()
    page.wait_for_function('typeof window.__home.releaseSelect === "function"')
    page.get_by_role('button', name='取消', exact=True).click()
    page.evaluate('window.__home.releaseSelect(); window.__home.pauseSelect=false')
    expect(page.get_by_role('dialog')).to_have_count(0)
    assert_feedback(page, check, 'profile cancellation ignores in-flight selection completion', 'info', '登录已取消')
    check('cancelled profile selection never persists', not page.evaluate("window.__home.calls.some(c=>c.command==='accounts_upsert')"))

    start()
    page.evaluate("window.__home.finish({status:'select',sessionId:'oauth_multi',profiles:[{id:'4'.repeat(32),name:'First'},{id:'5'.repeat(32),name:'Second'}]})")
    expect(page.get_by_role('dialog', name='选择角色')).to_be_visible()
    check('multiple roles do not report login success before selection', page.locator('.ni-toast').count() == 0)
    page.evaluate('window.__home.selectError=true')
    page.get_by_role('button', name='确认', exact=True).click()
    expect(page.get_by_role('dialog', name='选择角色')).to_be_visible()
    assert_feedback(page, check, 'profile failure is toast-only and remains retryable', 'error', '选角失败')
    clear_feedback(page)
    page.evaluate('window.__home.selectError=false')
    page.get_by_role('button', name='确认', exact=True).click()
    expect(page.get_by_role('dialog')).to_have_count(0)
    expect(page.locator('.home__name')).to_have_text('Selected Player')
    assert_feedback(page, check, 'multi-role success is notified exactly once after activation', 'success', 'Beta Skin 登录成功')

    page.goto(base_url + '/tests/feature-mod-oauth-ui/home.html')
    page.wait_for_load_state('networkidle')
    page.evaluate("window.__home.select('empty')")
    expect(button).to_be_enabled()
    start()
    page.evaluate("window.__home.finish({status:'account',account:{id:'single',type:'littleskin',name:'Single Player',uuid:'6'.repeat(32),server:'https://empty.example'}})")
    expect(page.get_by_role('dialog')).to_have_count(0)
    expect(page.locator('.home__name')).to_have_text('Single Player')
    assert_feedback(page, check, 'single-role success survives login component unmount', 'success', 'Beta Skin 登录成功')


def verify_login_feedback(page, check, base_url):
    def load(kind):
        page.goto(base_url + '/tests/feature-mod-oauth-ui/home.html?login=' + kind)
        page.wait_for_load_state('networkidle')
        clear_feedback(page)

    def msa_button():
        return page.locator('.home').get_by_role('button', name='使用 Microsoft 登录', exact=True)

    def start_msa():
        expect(msa_button()).to_be_enabled()
        msa_button().click()
        page.wait_for_function('typeof window.__home.rejectMsa === "function"')

    def skin_result():
        return {'status': 'account', 'account': {'id': 'new-skin', 'type': 'littleskin', 'name': 'Skin Player', 'uuid': '6'*32, 'server': 'https://empty.example'}}

    def msa_result():
        return {'status': 'success', 'account': {'id': 'new-msa', 'type': 'microsoft', 'name': 'Microsoft Player', 'uuid': '7'*32}}

    for message, tone in [('用户已取消登录', 'info'), ('设备代码已过期，请重新开始登录', 'error'), ('Xbox 认证失败', 'error')]:
        load('microsoft')
        start_msa()
        page.evaluate('(message)=>window.__home.rejectMsa(message)', message)
        expect(page.get_by_role('dialog')).to_have_count(0)
        assert_feedback(page, check, f'Microsoft terminal result is toast-only: {message}', tone, '登录已取消' if tone == 'info' else message)
        expect(msa_button()).to_be_enabled()
        polls = page.evaluate("window.__home.calls.filter(c=>c.command==='msa_device_poll').length")
        page.wait_for_timeout(1100)
        check(f'Microsoft stops polling after: {message}', polls == page.evaluate("window.__home.calls.filter(c=>c.command==='msa_device_poll').length"))

    for mode, message in [('start-error', '无法获取设备码'), ('expiry', '设备代码已过期')]:
        load('microsoft')
        page.evaluate('(mode)=>window.__home.msaMode=mode', mode)
        msa_button().click()
        expect(page.locator('.ni-toast')).to_have_count(1)
        expect(page.get_by_role('dialog')).to_have_count(0)
        assert_feedback(page, check, f'Microsoft {mode} closes modal and notifies once', 'error', message)

    for result in ['success', 'failure']:
        load('microsoft')
        start_msa()
        page.get_by_role('button', name='取消', exact=True).click()
        if result == 'success':
            page.evaluate('(result)=>window.__home.resolveMsa(result)', msa_result())
        else:
            page.evaluate("window.__home.rejectMsa('用户已取消登录')")
        expect(page.get_by_role('dialog')).to_have_count(0)
        page.wait_for_timeout(150)
        assert_feedback(page, check, f'Microsoft local cancel ignores late {result}', 'info', 'Microsoft 登录已取消')
        check(f'cancelled Microsoft {result} cannot persist', not page.evaluate("window.__home.calls.some(c=>c.command==='accounts_upsert')"))

    load('microsoft')
    page.evaluate("window.__home.msaMode='start-pending'")
    msa_button().click()
    page.wait_for_function('typeof window.__home.releaseMsaStart === "function"')
    page.get_by_role('button', name='取消', exact=True).click()
    page.evaluate('window.__home.releaseMsaStart()')
    expect(page.get_by_role('dialog')).to_have_count(0)
    page.wait_for_timeout(1100)
    assert_feedback(page, check, 'Microsoft cancellation during preparation ignores late device code', 'info', '登录已取消')
    check('cancelled Microsoft preparation never polls', not page.evaluate("window.__home.calls.some(c=>c.command==='msa_device_poll')"))

    load('microsoft')
    start_msa()
    page.evaluate('window.__home.openError=true')
    page.get_by_role('button', name='重新打开网页', exact=True).click()
    assert_feedback(page, check, 'browser open failure uses toast without ending device flow', 'error', '无法自动打开浏览器')
    expect(page.get_by_role('dialog')).to_be_visible()
    clear_feedback(page)
    page.evaluate("window.__home.rejectMsa('用户已取消登录')")
    expect(page.get_by_role('dialog')).to_have_count(0)
    assert_feedback(page, check, 'denial after browser error has no stale inline notification', 'info', '登录已取消')

    load('classic')
    form = page.locator('.home .skin-login')
    expect(form.get_by_role('button', name='下一步', exact=True)).to_be_disabled()
    form.get_by_placeholder('邮箱', exact=True).fill('fixture@example.test')
    form.get_by_role('button', name='下一步', exact=True).click()
    expect(form.get_by_role('button', name='登录', exact=True)).to_be_disabled()
    check('classic input validation stays local without result toast', page.locator('.ni-toast').count() == 0)
    form.get_by_placeholder('密码', exact=True).fill('fixture-password')
    form.get_by_role('button', name='登录', exact=True).click()
    page.wait_for_function('typeof window.__home.rejectClassic === "function"')
    page.evaluate("window.__home.rejectClassic('邮箱或密码不正确')")
    assert_feedback(page, check, 'classic login failure uses only one toast', 'error', '邮箱或密码不正确')
    clear_feedback(page)
    form.get_by_role('button', name='登录', exact=True).click()
    page.evaluate("window.__home.resolveClassic({status:'select',sessionId:'empty_roles',profiles:[]})")
    expect(form.locator('.account-form__error')).to_contain_text('没有可用角色')
    check('missing roles remains an actionable in-flow hint without duplicate toast', page.locator('.ni-toast').count() == 0)

    for kind in ['oauth', 'microsoft', 'classic']:
        for failure in ['none', 'write', 'activate', 'both']:
            load(kind)
            page.evaluate('''(failure)=>{
              window.__home.writeError=['write','both'].includes(failure);
              window.__home.activateError=['activate','both'].includes(failure);
              window.__home.pauseSave=true;
            }''', failure)
            if kind == 'microsoft':
                start_msa()
                page.evaluate('(result)=>window.__home.resolveMsa(result)', msa_result())
            elif kind == 'oauth':
                page.evaluate("window.__home.oauthMode='finish'")
                page.locator('.home .skin-oauth-login__button').click()
                page.wait_for_function('typeof window.__home.finish === "function"')
                page.evaluate('(result)=>window.__home.finish(result)', skin_result())
            else:
                form = page.locator('.home .skin-login')
                form.get_by_placeholder('邮箱', exact=True).fill('fixture@example.test')
                form.get_by_role('button', name='下一步', exact=True).click()
                form.get_by_placeholder('密码', exact=True).fill('fixture-password')
                form.get_by_role('button', name='登录', exact=True).click()
                page.wait_for_function('typeof window.__home.resolveClassic === "function"')
                page.evaluate('(result)=>window.__home.resolveClassic(result)', skin_result())
            page.wait_for_function('typeof window.__home.releaseSave === "function"')
            check(f'{kind}/{failure}: no premature success while save is pending', page.locator('.ni-toast').count() == 0)
            if kind == 'microsoft':
                expect(page.get_by_role('button', name='取消', exact=True)).to_be_disabled()
                page.keyboard.press('Escape')
                expect(page.get_by_role('dialog')).to_be_visible()
            page.evaluate('window.__home.releaseSave()')
            expect(page.get_by_role('dialog')).to_have_count(0)
            assert_feedback(page, check, f'{kind}/{failure}: final notification emitted once by Home', 'success' if failure == 'none' else 'error', '登录成功' if failure == 'none' else '未能写入')
            check(f'{kind}/{failure}: no duplicate persistence warning in Home', '未能写入' not in page.locator('.home').inner_text())

    load('oauth')
    page.evaluate("window.__home.oauthMode='finish'; window.__home.writeError=true")
    page.locator('.home .skin-oauth-login__button').click()
    page.wait_for_function('typeof window.__home.finish === "function"')
    page.evaluate("window.__home.finish({status:'select',sessionId:'save_failure_multi',profiles:[{id:'4'.repeat(32),name:'First'}]})")
    expect(page.get_by_role('dialog', name='选择角色')).to_be_visible()
    page.get_by_role('button', name='确认', exact=True).click()
    expect(page.get_by_role('dialog')).to_have_count(0)
    assert_feedback(page, check, 'multi-role persistence failure never also announces login success', 'error', '未能写入')
    page.screenshot(path='tests/feature-mod-oauth-ui/evidence/login-save-error.png')
