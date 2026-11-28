"""Toast exit, stacking, interruption and accessibility checks using real browser frames."""
import argparse
import json
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

parser = argparse.ArgumentParser()
parser.add_argument('--url', required=True)
args = parser.parse_args()
evidence = Path('tests/toast-ui/evidence')
evidence.mkdir(exist_ok=True)
checks = []
traces = {}


def check(name, condition):
    checks.append({'name': name, 'ok': bool(condition)})
    assert condition, name
    print('PASS:', name)


GEOMETRY = '''() => [...document.querySelectorAll('.ni-toast')].map(n=>{
  const r=n.getBoundingClientRect(), s=n.closest('.ni-toast-slot').getBoundingClientRect();
  return {id:Number(n.dataset.id),text:n.textContent,x:r.x,y:r.y,width:r.width,height:r.height,
    slotHeight:s.height,opacity:Number(getComputedStyle(n).opacity),leaving:n.dataset.leaving==='true'};
})'''

with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page(viewport={'width': 1100, 'height': 780})
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))

    def load():
        page.emulate_media(reduced_motion='no-preference')
        page.goto(args.url + '/tests/toast-ui/regression.html')
        page.wait_for_load_state('networkidle')

    def show(text, tone='info'):
        page.evaluate('([text,tone])=>window.__toast.showToast(text,tone)', [text, tone])
        return page.evaluate('window.__toast.messages.at(-1).id')

    def seed():
        page.evaluate('''() => {
          window.__toast.showToast('First notification stays at the top', 'info');
          window.__toast.showToast('Second notification is successful', 'success');
          window.__toast.showToast('Third notification reports an error', 'error');
        }''')
        expect(page.locator('.ni-toast')).to_have_count(3)
        page.wait_for_timeout(560)
        return page.evaluate(GEOMETRY)

    def sample_dismiss(index):
        return page.evaluate('''async (index) => {
          const read = ''' + GEOMETRY + ''';
          const before=read(), id=before[index].id, samples=[];
          window.__toast.dismissToast(id);
          const start=performance.now();
          await new Promise(resolve=>{
            const frame=()=>{
              const elapsed=performance.now()-start;
              samples.push({elapsed,items:read()});
              if(elapsed<550) requestAnimationFrame(frame); else resolve();
            };
            requestAnimationFrame(frame);
          });
          return {before,id,samples,after:read()};
        }''', index)

    load()
    initial = seed()
    check('three tones stack oldest above newest', [n['text'] for n in initial] == [
        'First notification stays at the top', 'Second notification is successful', 'Third notification reports an error'])
    check('stack aligns with left edge and bottom gutter', all(abs(n['x']) < 1 for n in initial) and abs(initial[-1]['y'] + initial[-1]['height'] - 760) < 1)
    check('stack maintains an eight pixel gap without overlap', all(abs(b['y'] - a['y'] - a['height'] - 8) < 1 for a, b in zip(initial, initial[1:])))
    page.mouse.click(80, initial[-1]['y'] + 15)
    check('toasts do not intercept the underlying control', page.evaluate('window.__toast.clicks') == 1)
    page.screenshot(path=str(evidence / 'three-toast-stack.png'))

    for index, label in [(1, 'middle'), (2, 'bottom'), (0, 'top')]:
        load()
        before = seed()
        trace = sample_dismiss(index)
        traces[label] = trace
        gone = [n for f in trace['samples'] for n in f['items'] if n['id'] == trace['id']]
        check(f'{label}: retained message visibly fades and retracts left', any(0.02 < n['opacity'] < 0.98 and n['x'] < -2 for n in gone))
        check(f'{label}: withdrawal precedes stack collapse', any(n['opacity'] < 0.5 and abs(n['slotHeight'] - before[index]['slotHeight']) < 1 for n in gone))
        check(f'{label}: occupied height collapses smoothly', any(1 < n['slotHeight'] < before[index]['slotHeight'] - 1 for n in gone))
        check(f'{label}: removed after exit with survivors intact', len(trace['after']) == 2 and all(n['id'] != trace['id'] for n in trace['after']))
        survivors = trace['after']
        check(f'{label}: survivors finish without a ghost gap', abs(survivors[-1]['y'] + survivors[-1]['height'] - 760) < 1 and abs(survivors[1]['y'] - survivors[0]['y'] - survivors[0]['height'] - 8) < 1)
        if index > 0:
            first_id = before[0]['id']
            positions = [n['y'] for f in trace['samples'] for n in f['items'] if n['id'] == first_id]
            check(f'{label}: surviving rows move continuously rather than jump', any(before[0]['y'] + 1 < y < survivors[0]['y'] - 1 for y in positions))
        else:
            check('top: removing oldest does not shift later notifications', all(abs(n['y'] - before[i + 1]['y']) < 1 for i, n in enumerate(survivors)))

    load()
    show('Auto', 'success')
    page.wait_for_function("document.querySelector('.ni-toast[data-leaving=true]')", timeout=4000)
    check('automatic expiry follows the shared exit path', page.locator('.ni-toast').count() == 1 and page.evaluate('window.__toast.messages.length') == 0)
    expect(page.locator('.ni-toast-host')).to_have_count(0)
    check('last toast removes the empty host', True)

    load()
    initial = seed()
    old_id = initial[1]['id']
    page.evaluate("window.__originalToast=document.querySelectorAll('.ni-toast')[1]; window.__toast.showToast('  Second notification is successful  ', 'error')")
    page.wait_for_timeout(50)
    renewed = page.evaluate(GEOMETRY)
    check('deduplication refreshes in place without replaying entry', len(renewed) == 3 and renewed[1]['text'] == initial[1]['text'] and renewed[1]['opacity'] == 1 and abs(renewed[1]['y'] - initial[1]['y']) < 1)
    check('deduplication updates tone and retains the same DOM node', page.evaluate("window.__originalToast === document.querySelectorAll('.ni-toast')[1] && window.__originalToast.dataset.tone === 'error'"))
    page.evaluate('(id)=>window.__toast.dismissToast(id)', old_id)
    expect(page.locator('.ni-toast')).to_have_count(3)
    check('stale dismissal cannot remove refreshed notification', page.locator('.ni-toast[data-leaving=true]').count() == 0)
    page.evaluate("window.__toast.dismissToast(Number(window.__originalToast.dataset.id))")
    page.wait_for_timeout(80)
    page.evaluate("window.__toast.showToast('Second notification is successful', 'success')")
    page.wait_for_timeout(600)
    expect(page.locator('.ni-toast')).to_have_count(3)
    check('refresh during withdrawal cancels old cleanup and reverses exit', page.evaluate("window.__originalToast.isConnected && getComputedStyle(window.__originalToast).opacity==='1' && !window.__originalToast.dataset.leaving"))

    load()
    show('Renew')
    page.wait_for_timeout(1500)
    show('Renew', 'success')
    page.wait_for_timeout(1000)
    check('duplicate resets lifetime beyond the original deadline', page.locator('.ni-toast:not([data-leaving])').count() == 1)
    expect(page.locator('.ni-toast-host')).to_have_count(0, timeout=3500)
    check('renewed lifetime eventually clears without orphan timer', True)

    load()
    page.evaluate("for(let i=1;i<=6;i++) window.__toast.showToast('Stack notification '+i)")
    page.wait_for_timeout(560)
    show('Stack notification 7')
    expect(page.locator('.ni-toast[data-leaving=true]')).to_have_count(1)
    check('capacity eviction uses exit animation with six active messages', page.evaluate('window.__toast.messages.length') == 6 and page.locator('.ni-toast').count() == 7)
    page.wait_for_timeout(600)
    check('capacity keeps the newest six in FIFO order', page.locator('.ni-toast__text').all_text_contents() == [f'Stack notification {i}' for i in range(2, 8)])
    page.screenshot(path=str(evidence / 'six-toast-stack.png'))
    page.evaluate('window.__toast.messages.forEach(m=>window.__toast.dismissToast(m.id))')
    expect(page.locator('.ni-toast[data-leaving=true]')).to_have_count(6)
    expect(page.locator('.ni-toast-host')).to_have_count(0)
    check('simultaneous dismissals leave no rows or gaps', True)

    load()
    for i in range(18):
        show(f'Burst notification {i}')
        page.wait_for_timeout(45)
    page.wait_for_timeout(600)
    check('rapid overflow stays bounded after exits settle', page.locator('.ni-toast').count() == 6 and page.evaluate('window.__toast.messages.length') == 6)
    geometry = page.evaluate(GEOMETRY)
    check('rapid overflow preserves visual order without overlap', all(b['y'] >= a['y'] + a['height'] for a, b in zip(geometry, geometry[1:])))

    for theme in ['light', 'dark']:
        load()
        page.set_viewport_size({'width': 360, 'height': 780})
        page.evaluate('(theme)=>document.documentElement.dataset.theme=theme', theme)
        show('A long notification wraps to multiple lines and remains readable while another notification is displayed.', 'error')
        show('Short message', 'info')
        page.wait_for_timeout(600)
        geometry = page.evaluate(GEOMETRY)
        check(f'{theme}: long text wraps inside narrow viewport', geometry[0]['height'] > 30 and all(n['width'] <= 340 for n in geometry))
        check(f'{theme}: wrapped toast does not overlap neighboring toast', abs(geometry[1]['y'] - geometry[0]['y'] - geometry[0]['height'] - 8) < 1)
        page.screenshot(path=str(evidence / f'narrow-{theme}.png'))

    page.set_viewport_size({'width': 1100, 'height': 780})
    load()
    seed()
    page.emulate_media(reduced_motion='reduce')
    page.evaluate('window.__toast.messages.forEach(m=>window.__toast.dismissToast(m.id))')
    page.wait_for_timeout(50)
    check('reduced motion removes every toast without waiting for CSS events', page.locator('.ni-toast-host').count() == 0)
    show('Reduced motion')
    expect(page.locator('.ni-toast')).to_have_css('opacity', '1')
    check('reduced motion disables entry and stack transitions', page.locator('.ni-toast').evaluate("n=>getComputedStyle(n).transitionDuration==='0s'") and page.locator('.ni-toast-slot').evaluate("n=>getComputedStyle(n).transitionDuration==='0s'"))

    load()
    seed()
    page.evaluate('window.__toast.messages.forEach(m=>window.__toast.dismissToast(m.id))')
    page.wait_for_timeout(60)
    page.emulate_media(reduced_motion='reduce')
    page.wait_for_timeout(60)
    check('enabling reduced motion mid-exit immediately clears retained rows', page.locator('.ni-toast-host').count() == 0)

    load()
    show('Interrupted')
    page.wait_for_timeout(60)
    page.evaluate('window.__toast.dismissToast(window.__toast.messages[0].id)')
    expect(page.locator('.ni-toast-host')).to_have_count(0)
    check('dismiss during entry cannot leave a stuck invisible slot', True)

    load()
    seed()
    page.add_style_tag(content='.ni-toast, .ni-toast-slot { transition: none !important; }')
    page.evaluate('window.__toast.messages.forEach(m=>window.__toast.dismissToast(m.id))')
    expect(page.locator('.ni-toast-host')).to_have_count(0)
    check('fallback cleanup works when transitionend never fires', True)

    load()
    seed()
    page.evaluate('window.__toast.setMounted(false)')
    expect(page.locator('.ni-toast-host')).to_have_count(0)
    page.evaluate('window.__toast.messages.forEach(m=>window.__toast.dismissToast(m.id)); window.__toast.setMounted(true)')
    show('Remounted')
    expect(page.locator('.ni-toast')).to_have_count(1)
    page.wait_for_timeout(600)
    check('host remount has no stale exit timers or retained messages', page.locator('.ni-toast__text').all_text_contents() == ['Remounted'])
    check('toast fixture has no runtime errors', not errors)
    browser.close()

(evidence / 'animation-frames.json').write_text(json.dumps(traces, indent=2), encoding='utf-8')
(evidence / 'report.json').write_text(json.dumps(checks, indent=2), encoding='utf-8')
print(f'{len(checks)} toast checks passed')
