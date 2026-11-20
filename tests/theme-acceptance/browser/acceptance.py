import argparse
import hashlib
import json
import os
import re
import struct
import sys
import traceback
import zlib
from datetime import datetime, timezone
from pathlib import Path

from playwright.sync_api import sync_playwright

ORDER = ["mono", "emerald", "blue", "violet", "amber", "rose", "nickel", "copper", "abyss", "paper", "fortyTwo", "custom"]
LABELS = ["黑与白", "翡翠绿", "海蓝色", "魅紫色", "琥珀色", "玫瑰红", "镍光", "铜霞", "深海", "纸灯", "四十二", "自定义"]
DEFAULT_CUSTOM = {"hue": 174, "saturation": 39, "lightness": 44, "titlebarHueShift": 0, "decoration": "lattice", "decorationOpacity": 0.35}
SLIDERS = {"hue": "主题色相", "saturation": "主题饱和度", "lightness": "主题明度", "titlebarHueShift": "顶栏色相偏移", "decorationOpacity": "装饰强度"}
TRACKER = """(() => {
  const schedule = window.setTimeout.bind(window);
  const cancel = window.clearTimeout.bind(window);
  const active = new Set();
  let created = 0, fired = 0, maximum = 0;
  window.setTimeout = (callback, delay, ...args) => {
    if (delay !== 500 || typeof callback !== 'function') return schedule(callback, delay, ...args);
    created += 1;
    const id = schedule(() => { active.delete(id); fired += 1; callback(...args); }, delay);
    active.add(id); maximum = Math.max(maximum, active.size); return id;
  };
  window.clearTimeout = id => { active.delete(id); return cancel(id); };
  window.__themeTimers = { snapshot: () => ({active: active.size, created, fired, maximum}) };
})();"""
COLORS = """() => {
  const root = getComputedStyle(document.documentElement);
  const cssColor = value => {
    const e = document.createElement('span'); e.style.color = value; document.body.append(e);
    const result = getComputedStyle(e).color; e.remove(); return result;
  };
  const props = ['--bg','--surface','--surface-2','--surface-3','--surface-4','--text','--text-dim','--accent','--accent-strong','--accent-fg','--accent-soft','--accent-ring','--accent-text'];
  const tokens = Object.fromEntries(props.map(key => [key, cssColor(root.getPropertyValue(key))]));
  const sample = selector => {
    const element = document.querySelector(selector);
    if (!element) return null;
    const style = getComputedStyle(element), box = element.getBoundingClientRect();
    return {color: style.color, background: style.backgroundColor, opacity: style.opacity, x: box.x, y: box.y, width: box.width, height: box.height};
  };
  return {tokens, primary: sample('[data-testid="primary-button"]'), signal: sample('[data-testid="signal-text"]'), body: sample('.ta-body-text'), titlebar: sample('.ni-titlebar'), brand: sample('.ni-brand__text'), nav: sample('.ni-navtab[data-active="true"]'), pill: sample('.ni-navtab__pill'), inactive: sample('.ni-navtab:not([data-active="true"])'), window: sample('.ni-winbtn'), segment: sample('.ni-seg__item[data-active="true"]'), segmentPill: sample('.ni-seg__pill'), previewButton: sample('.st-preview__btn--accent'), previewTitle: sample('.st-preview__bar-title'), previewBar: sample('.st-preview__bar')};
}"""


def rgba(value):
    if value.startswith("#"):
        return tuple(int(value[index:index + 2], 16) for index in (1, 3, 5)) + (1.0,)
    if value.startswith("color(srgb"):
        parts = [float(number) for number in re.findall(r"[-\d.]+", value)]
        return tuple(number * 255 for number in parts[:3]) + (parts[3] if len(parts) > 3 else 1.0,)
    parts = [float(number) for number in re.findall(r"[-\d.]+", value)]
    if len(parts) < 3:
        raise ValueError("Unsupported CSS color: " + value)
    return tuple(parts[:3]) + (parts[3] if len(parts) > 3 else 1.0,)


def blend(foreground, background):
    f, b = rgba(foreground), rgba(background) if isinstance(background, str) else background
    return tuple(f[index] * f[3] + b[index] * (1 - f[3]) for index in range(3)) + (1.0,)


def luminance(color):
    values = [(channel / 255 / 12.92) if channel / 255 <= 0.04045 else ((channel / 255 + 0.055) / 1.055) ** 2.4 for channel in color[:3]]
    return sum(channel * weight for channel, weight in zip(values, (0.2126, 0.7152, 0.0722)))


def contrast(foreground, background):
    a = rgba(foreground) if isinstance(foreground, str) else foreground
    b = rgba(background) if isinstance(background, str) else background
    la, lb = luminance(a), luminance(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def png_pixels(data):
    position, chunks, width, height, channels = 8, [], 0, 0, 0
    while position < len(data):
        size = struct.unpack(">I", data[position:position + 4])[0]
        kind = data[position + 4:position + 8]
        payload = data[position + 8:position + 8 + size]
        position += size + 12
        if kind == b"IHDR":
            width, height, depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", payload)
            if depth != 8 or color_type not in (2, 6) or interlace:
                raise ValueError("Unsupported screenshot PNG layout")
            channels = 3 if color_type == 2 else 4
        elif kind == b"IDAT":
            chunks.append(payload)
    raw = zlib.decompress(b"".join(chunks))
    stride, prior, result, cursor = width * channels, bytearray(width * channels), [], 0
    for _ in range(height):
        filter_type = raw[cursor]
        row = bytearray(raw[cursor + 1:cursor + 1 + stride])
        cursor += stride + 1
        for index in range(stride):
            left = row[index - channels] if index >= channels else 0
            up = prior[index]
            upper_left = prior[index - channels] if index >= channels else 0
            if filter_type == 1:
                predictor = left
            elif filter_type == 2:
                predictor = up
            elif filter_type == 3:
                predictor = (left + up) // 2
            elif filter_type == 4:
                p = left + up - upper_left
                distances = (abs(p - left), abs(p - up), abs(p - upper_left))
                predictor = (left, up, upper_left)[distances.index(min(distances))]
            elif filter_type == 0:
                predictor = 0
            else:
                raise ValueError("Unsupported PNG filter")
            row[index] = (row[index] + predictor) & 255
        result.extend(tuple(row[index:index + 3]) for index in range(0, stride, channels))
        prior = row
    return width, height, result


def pixel_difference(a, b):
    width, height, pixels_a = png_pixels(a)
    width_b, height_b, pixels_b = png_pixels(b)
    if (width, height) != (width_b, height_b):
        raise AssertionError("Screenshot dimensions changed")
    deltas = [max(abs(x - y) for x, y in zip(pa, pb)) for pa, pb in zip(pixels_a, pixels_b)]
    return {"width": width, "height": height, "changedPixels": sum(delta >= 2 for delta in deltas), "maxChannelDelta": max(deltas, default=0)}


class Acceptance:
    def __init__(self, browser, base, evidence):
        self.browser, self.base, self.evidence = browser, base.rstrip("/"), evidence
        self.results, self.errors, self.screenshots, self.contexts = [], [], [], []
        self.started = datetime.now(timezone.utc).isoformat()
        self.evidence.mkdir(parents=True, exist_ok=True)

    def check(self, name, condition, details=None):
        self.results.append({"name": name, "passed": bool(condition), "details": details})
        print(("PASS " if condition else "FAIL ") + name, flush=True)

    def new_page(self, seed=None, route="settings", tauri=False, mode="dark", motion="no-preference", viewport=None):
        context = self.browser.new_context(viewport=viewport or {"width": 1280, "height": 960}, color_scheme=mode, reduced_motion=motion)
        self.contexts.append(context)
        page = context.new_page()
        page.add_init_script(TRACKER)
        if seed is not None:
            value = json.dumps(seed, ensure_ascii=True)
            page.add_init_script("if (!sessionStorage.getItem('theme.acceptance.seeded')) { localStorage.setItem('nickel.settings', JSON.stringify(" + value + ")); sessionStorage.setItem('theme.acceptance.seeded', 'true'); }")
        page.on("pageerror", lambda error: self.errors.append({"type": "pageerror", "message": str(error)}))
        page.on("console", lambda message: self.errors.append({"type": "console", "message": message.text}) if message.type == "error" else None)
        page.set_default_timeout(15000)
        page.goto(self.base + "/tests/theme-acceptance/fixtures/app.html?route=" + route + ("&host=tauri" if tauri else ""), wait_until="domcontentloaded", timeout=60000)
        page.wait_for_load_state("networkidle", timeout=60000)
        page.wait_for_function("document.documentElement.dataset.fixtureReady === 'true'", timeout=60000)
        page.wait_for_timeout(480)
        return page

    def patch(self, page, patch):
        page.evaluate("patch => window.__themeAcceptance.update(patch)", patch)
        page.wait_for_function("patch => Object.entries(patch).every(([key, value]) => JSON.stringify(window.__themeAcceptance.settings[key]) === JSON.stringify(value) && JSON.stringify(window.__themeAcceptance.saved()[key]) === JSON.stringify(value))", arg=patch)
        page.wait_for_timeout(440 if patch.get("accent") in ORDER[:10] or "themeMode" in patch else 40)

    def screenshot(self, page, name):
        page.mouse.move(0, 0)
        page.wait_for_timeout(220)
        path = self.evidence / (name + ".png")
        page.screenshot(path=str(path), full_page=True)
        self.screenshots.append({"path": str(path.resolve()), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})

    def run_section(self, name, callback):
        try:
            callback()
        except Exception as error:
            self.check(name + " completes", False, {"error": str(error), "traceback": traceback.format_exc()})

    def palette(self):
        page = self.new_page()
        group = page.get_by_role("radiogroup", name="颜色主题", exact=True)
        radios = group.get_by_role("radio")
        self.check("palette contains twelve options in exact order", radios.all_text_contents() == LABELS, radios.all_text_contents())
        self.check("custom sidebar hidden by default", page.get_by_role("navigation", name="设置分区").get_by_role("button", name="自定义", exact=True).count() == 0)
        self.check("titlebar tint defaults off", page.evaluate("window.__themeAcceptance.settings.titlebarTint") is False)
        for mode in ("dark", "light"):
            self.patch(page, {"themeMode": mode})
            for accent, label in zip(ORDER, LABELS):
                group.get_by_role("radio", name=label, exact=True).click()
                page.wait_for_function("accent => window.__themeAcceptance.saved().accent === accent && document.documentElement.dataset.accent === accent", arg=accent)
                state = group.evaluate("e => Array.from(e.querySelectorAll('[role=radio]')).map(r => ({name: r.getAttribute('aria-label'), checked: r.getAttribute('aria-checked'), tab: r.tabIndex}))")
                active = [entry for entry in state if entry["checked"] == "true"]
                self.check("palette selects and saves " + mode + "/" + accent, len(active) == 1 and active[0]["name"] == label and sum(entry["tab"] == 0 for entry in state) == 1, active)
                self.check("custom sidebar condition " + mode + "/" + accent, page.get_by_role("navigation", name="设置分区").get_by_role("button", name="自定义", exact=True).count() == (1 if accent == "custom" else 0))
        self.patch(page, {"accent": "mono"})
        group.get_by_role("radio", name=LABELS[0], exact=True).focus()
        page.keyboard.press("ArrowLeft")
        page.wait_for_function("window.__themeAcceptance.settings.accent === 'custom'")
        self.check("palette keyboard left wraps to custom and moves focus", page.evaluate("document.activeElement.getAttribute('aria-label')") == "自定义")
        page.keyboard.press("Home")
        page.wait_for_function("window.__themeAcceptance.settings.accent === 'mono'")
        self.check("palette Home selects first", page.evaluate("document.activeElement.getAttribute('aria-label')") == "黑与白")
        for key, expected in [("ArrowRight", "emerald"), ("ArrowDown", "blue"), ("ArrowUp", "emerald"), ("End", "custom")]:
            page.keyboard.press(key)
            page.wait_for_function("accent => window.__themeAcceptance.settings.accent === accent", arg=expected)
            self.check("palette keyboard " + key, page.evaluate("document.activeElement.getAttribute('aria-label')") == LABELS[ORDER.index(expected)])
        page.keyboard.press("Space")
        self.check("palette native Space remains selected", page.evaluate("window.__themeAcceptance.settings.accent") == "custom")
        self.screenshot(page, "palette-light")
        page.reload(wait_until="domcontentloaded", timeout=60000)
        page.wait_for_load_state("networkidle")
        page.wait_for_function("document.documentElement.dataset.fixtureReady === 'true'")
        self.check("palette selection persists through reload", page.evaluate("window.__themeAcceptance.settings.accent") == "custom")
        legacy = {
            "mono": ("#f2f4f5", "#d8dcdd", "#0e1011"), "emerald": ("#3ecf8e", "#2bb87b", "#05170f"),
            "blue": ("#5a97e8", "#3f7fd0", "#04101f"), "violet": ("#9b8cf0", "#7c68e0", "#120a2a"),
            "amber": ("#e0aa4c", "#c08f2f", "#1d1403"), "rose": ("#ea6f8b", "#d35070", "#2a0712"),
        }
        for mode in ("dark", "light"):
            for accent, expected in legacy.items():
                self.patch(page, {"accent": accent, "themeMode": mode})
                tokens = page.evaluate(COLORS)["tokens"]
                expected = ("#14171a", "#2b2f31", "#f5f6f7") if mode == "light" and accent == "mono" else expected
                measured = [rgba(tokens[key])[:3] for key in ("--accent", "--accent-strong", "--accent-fg")]
                self.check("legacy CSS tokens unchanged " + mode + "/" + accent, measured == [rgba(color)[:3] for color in expected], {"actual": measured, "expected": expected})
                self.check("legacy has no new SVG artwork " + mode + "/" + accent, page.locator(".ni-theme-artwork").count() == 0)

    def custom(self):
        page = self.new_page({"accent": "custom", "themeMode": "dark"})
        nav = page.get_by_role("navigation", name="设置分区")
        nav.get_by_role("button", name="自定义", exact=True).click()
        page.get_by_role("slider", name="主题色相").wait_for()
        page.wait_for_timeout(400)
        self.check("custom is an independent panel", page.get_by_role("radiogroup", name="颜色主题").count() == 0 and page.get_by_role("slider").count() == 5)
        shift = page.get_by_role("slider", name=SLIDERS["titlebarHueShift"])
        self.check("titlebar shift disabled with tint off", shift.is_disabled())
        for field, lo, hi in [("hue", 0, 360), ("saturation", 0, 100), ("lightness", 0, 100)]:
            slider = page.get_by_role("slider", name=SLIDERS[field])
            self.check("custom slider bounds " + field, slider.get_attribute("min") == str(lo) and slider.get_attribute("max") == str(hi))
            for key, expected in (("Home", lo), ("End", hi)):
                slider.focus()
                page.keyboard.press(key)
                page.wait_for_function("args => window.__themeAcceptance.saved().customTheme[args.field] === args.value", arg={"field": field, "value": expected})
                self.check("custom keyboard boundary persists " + field + "/" + key, int(slider.input_value()) == expected, page.evaluate("window.__themeAcceptance.saved().customTheme"))
        decoration = page.get_by_role("radiogroup", name="原创装饰样式")
        self.check("all five custom decorations exposed", decoration.get_by_role("radio").evaluate_all("items => items.map(i => i.value)") == ["none", "lattice", "ribbons", "orbits", "folds"])
        decoration.get_by_role("radio", name="晶格").focus()
        for expected in ("ribbons", "orbits", "folds", "none", "lattice"):
            page.keyboard.press("ArrowRight")
            page.wait_for_function("value => window.__themeAcceptance.saved().customTheme.decoration === value", arg=expected)
            self.check("custom native radio keyboard saves " + expected, page.evaluate("document.activeElement.value") == expected)
        decoration.get_by_role("radio", name="无装饰").check()
        page.wait_for_function("window.__themeAcceptance.settings.customTheme.decoration === 'none'")
        self.check("none disables intensity and removes shell artwork", page.get_by_role("slider", name=SLIDERS["decorationOpacity"]).is_disabled() and page.locator(".ni-bg .ni-theme-artwork, .ni-titlebar > .ni-theme-artwork").count() == 0)
        decoration.get_by_role("radio", name="流带").check()
        intensity = page.get_by_role("slider", name=SLIDERS["decorationOpacity"])
        for key, expected in (("Home", 0), ("End", 1)):
            intensity.focus()
            page.keyboard.press(key)
            page.wait_for_function("value => window.__themeAcceptance.saved().customTheme.decorationOpacity === value", arg=expected)
            opacity = page.locator(".ni-titlebar > .ni-theme-artwork").evaluate("e => getComputedStyle(e).opacity")
            self.check("custom opacity applies and saves " + key, float(opacity) == expected, opacity)
        page.get_by_role("navigation", name="主导航").get_by_role("button", name="更多", exact=True).click()
        page.get_by_role("navigation", name="更多分区").get_by_role("button", name="前沿计划", exact=True).click()
        switch = page.get_by_role("switch", name="顶栏颜色小改", exact=True)
        switch.wait_for()
        self.check("frontier shows UI group and default-off switch", page.get_by_text("用户界面项", exact=True).count() == 1 and switch.get_attribute("aria-checked") == "false")
        before = page.locator(".ni-titlebar").evaluate("e => getComputedStyle(e).backgroundColor")
        switch.focus()
        page.keyboard.press("Space")
        page.wait_for_function("window.__themeAcceptance.saved().titlebarTint === true")
        after = page.locator(".ni-titlebar").evaluate("e => getComputedStyle(e).backgroundColor")
        self.check("frontier keyboard enables tinted translucent titlebar", before != after and rgba(after)[3] < 1 and page.locator(".ni-titlebar").get_attribute("data-tinted") == "true", {"off": before, "on": after})
        self.screenshot(page, "frontier-dark")
        self.patch(page, {"themeMode": "light"})
        self.screenshot(page, "frontier-light")
        page.get_by_role("navigation", name="主导航").get_by_role("button", name="设置", exact=True).click()
        nav = page.get_by_role("navigation", name="设置分区")
        nav.get_by_role("button", name="自定义", exact=True).click()
        shift = page.get_by_role("slider", name=SLIDERS["titlebarHueShift"])
        shift.wait_for()
        self.check("titlebar shift enabled by frontier setting", shift.is_enabled())
        color_before = page.evaluate("getComputedStyle(document.documentElement).getPropertyValue('--accent')")
        for key, value in (("Home", -180), ("End", 180)):
            shift.focus()
            page.keyboard.press(key)
            page.wait_for_function("value => window.__themeAcceptance.saved().customTheme.titlebarHueShift === value", arg=value)
            self.check("titlebar shift keyboard saves boundary " + key, int(shift.input_value()) == value)
        self.check("titlebar shift does not recolor primary", page.evaluate("getComputedStyle(document.documentElement).getPropertyValue('--accent')") == color_before)
        parameters = {"hue": 286, "saturation": 72, "lightness": 45, "titlebarHueShift": 61, "decoration": "orbits", "decorationOpacity": 0.68}
        self.patch(page, {"customTheme": parameters})
        self.screenshot(page, "custom-light")
        self.patch(page, {"themeMode": "dark"})
        self.screenshot(page, "custom-dark")
        nav.get_by_role("button", name="外观", exact=True).click()
        page.get_by_role("radiogroup", name="颜色主题").wait_for()
        self.check("leaving custom panel keeps parameters", page.evaluate("window.__themeAcceptance.saved().customTheme") == parameters)
        page.reload(wait_until="domcontentloaded", timeout=60000)
        page.wait_for_load_state("networkidle")
        page.wait_for_function("document.documentElement.dataset.fixtureReady === 'true'")
        self.check("custom parameters and tint survive browser reload", page.evaluate("window.__themeAcceptance.settings.customTheme") == parameters and page.evaluate("window.__themeAcceptance.settings.titlebarTint") is True)
        nav = page.get_by_role("navigation", name="设置分区")
        nav.get_by_role("button", name="自定义", exact=True).click()
        page.get_by_role("button", name="重置自定义参数", exact=True).click()
        page.wait_for_function("value => JSON.stringify(window.__themeAcceptance.saved().customTheme) === JSON.stringify(value)", arg=DEFAULT_CUSTOM)
        self.check("reset restores all six fields without changing tint or selected theme", page.evaluate("window.__themeAcceptance.settings.customTheme") == DEFAULT_CUSTOM and page.evaluate("window.__themeAcceptance.settings.titlebarTint") is True and page.evaluate("window.__themeAcceptance.settings.accent") == "custom")
        self.patch(page, {"accent": "nickel"})
        page.get_by_role("radiogroup", name="颜色主题").wait_for()
        page.wait_for_timeout(400)
        self.check("removing custom selection falls back to Appearance", nav.get_by_role("button", name="自定义", exact=True).count() == 0 and nav.get_by_role("button", name="外观", exact=True).get_attribute("data-active") == "true" and page.get_by_role("slider", name="主题色相").count() == 0)
        page.get_by_role("navigation", name="主导航").get_by_role("button", name="更多", exact=True).click()
        page.get_by_role("navigation", name="更多分区").get_by_role("button", name="前沿计划", exact=True).click()
        switch = page.get_by_role("switch", name="顶栏颜色小改", exact=True)
        switch.click()
        page.wait_for_function("window.__themeAcceptance.saved().titlebarTint === false")
        off = page.locator(".ni-titlebar").evaluate("e => getComputedStyle(e).backgroundColor")
        expected_off = page.evaluate("getComputedStyle(document.documentElement).getPropertyValue('--chrome-bg')")
        self.check("turning tint off restores neutral chrome", rgba(off) == rgba(expected_off), {"actual": off, "expected": expected_off})

    def visual_and_contrast(self):
        page = self.new_page({"accent": "nickel", "themeMode": "dark", "titlebarTint": True}, route="home")
        fingerprints, title_fingerprints, color_metrics = {}, {}, []
        for mode in ("dark", "light"):
            for accent in ORDER[6:11]:
                self.patch(page, {"accent": accent, "themeMode": mode})
                art = page.locator("[data-testid=art-stage] > .ni-theme-artwork")
                info = art.evaluate("e => ({motif: e.dataset.motif, width: e.getBoundingClientRect().width, height: e.getBoundingClientRect().height, opacity: getComputedStyle(e).opacity, pointer: getComputedStyle(e).pointerEvents, paths: Array.from(e.querySelectorAll('path,ellipse,circle')).map(n => n.outerHTML).join(''), images: e.querySelectorAll('image,foreignObject,use').length, childrenNoninteractive: Array.from(e.querySelectorAll('*')).every(n => getComputedStyle(n).pointerEvents === 'none')})")
                fingerprints[accent] = hashlib.sha256(re.sub(r"theme-glow-[^)]+", "theme-glow", info["paths"]).encode()).hexdigest()
                self.check("original background geometry visible and noninteractive " + mode + "/" + accent, info["motif"] == accent and info["width"] > 0 and info["height"] > 0 and float(info["opacity"]) > 0 and info["pointer"] == "none" and info["childrenNoninteractive"] and info["images"] == 0, info)
                title_art = page.locator(".ni-titlebar > .ni-theme-artwork")
                title_info = title_art.evaluate("e => ({html: e.innerHTML, display: getComputedStyle(e).display, pointer: getComputedStyle(e).pointerEvents, width: e.getBoundingClientRect().width})")
                title_fingerprints[accent] = hashlib.sha256(title_info["html"].encode()).hexdigest()
                self.check("original titlebar geometry visible " + mode + "/" + accent, title_info["display"] != "none" and title_info["width"] > 0 and title_info["pointer"] == "none")
                ids = page.locator(".ni-theme-artwork radialGradient").evaluate_all("items => items.map(e => e.id)")
                valid_refs = page.evaluate("Array.from(document.querySelectorAll('.ni-theme-artwork [fill^=\"url\"]')).every(e => { const id = e.getAttribute('fill').slice(5,-1); return e.closest('svg').querySelector(`[id=\"${id}\"]`) !== null; })")
                self.check("all gradient IDs unique and locally resolved " + mode + "/" + accent, len(ids) == len(set(ids)) and len(ids) >= 3 and valid_refs, ids)
                if accent == "fortyTwo":
                    page.emulate_media(reduced_motion="reduce")
                    page.wait_for_function("document.documentElement.dataset.themeMotion === 'reduced'")
                stage = page.get_by_test_id("art-stage")
                with_art = stage.screenshot(animations="disabled")
                art.evaluate("e => e.style.visibility = 'hidden'")
                without_art = stage.screenshot(animations="disabled")
                art.evaluate("e => e.style.removeProperty('visibility')")
                difference = pixel_difference(with_art, without_art)
                self.check("background produces real rendered pixels " + mode + "/" + accent, difference["changedPixels"] > 25 and difference["maxChannelDelta"] >= 2, difference)
                if accent == "fortyTwo":
                    page.emulate_media(reduced_motion="no-preference")
                metrics = self.measure_contrast(page, mode + "/" + accent)
                color_metrics.append(metrics)
                self.screenshot(page, accent + "-" + mode)
        self.check("five built-in backgrounds are distinct compositions", len(set(fingerprints.values())) == 5, fingerprints)
        self.check("five built-in titlebar decorations are distinct compositions", len(set(title_fingerprints.values())) == 5, title_fingerprints)
        cases = [
            {"hue": 0, "saturation": 0, "lightness": 0}, {"hue": 360, "saturation": 100, "lightness": 100},
            {"hue": 0, "saturation": 100, "lightness": 50}, {"hue": 60, "saturation": 100, "lightness": 50},
            {"hue": 120, "saturation": 100, "lightness": 50}, {"hue": 180, "saturation": 100, "lightness": 50},
            {"hue": 240, "saturation": 100, "lightness": 50}, {"hue": 300, "saturation": 100, "lightness": 50},
            {"hue": 174, "saturation": 0, "lightness": 20}, {"hue": 174, "saturation": 0, "lightness": 80},
        ]
        for mode in ("dark", "light"):
            for index, case in enumerate(cases):
                parameters = {**DEFAULT_CUSTOM, **case, "titlebarHueShift": -180 if index % 2 == 0 else 180, "decorationOpacity": index % 2}
                self.patch(page, {"accent": "custom", "themeMode": mode, "customTheme": parameters})
                color_metrics.append(self.measure_contrast(page, mode + "/custom-" + str(index)))
        (self.evidence / "contrast.json").write_text(json.dumps(color_metrics, ensure_ascii=False, indent=2), encoding="utf-8")

    def measure_contrast(self, page, label):
        page.mouse.move(0, 0)
        page.wait_for_timeout(200)
        snapshot = page.evaluate(COLORS)
        tokens = snapshot["tokens"]
        chrome = blend(snapshot["titlebar"]["background"], tokens["--bg"])
        ratios = {
            "primary": contrast(snapshot["primary"]["color"], snapshot["primary"]["background"]),
            "primaryStrong": contrast(tokens["--accent-fg"], tokens["--accent-strong"]),
            "activeNavigation": contrast(snapshot["nav"]["color"], snapshot["pill"]["background"]),
            "selectedSegment": contrast(snapshot["segment"]["color"], snapshot["segmentPill"]["background"]),
            "previewButton": contrast(snapshot["previewButton"]["color"], snapshot["previewButton"]["background"]),
            "body": contrast(snapshot["body"]["color"], tokens["--surface"]),
            "brand": contrast(snapshot["brand"]["color"], chrome),
            "inactiveNavigation": contrast(snapshot["inactive"]["color"], chrome),
            "windowControl": contrast(snapshot["window"]["color"], chrome),
            "previewTitle": contrast(snapshot["previewTitle"]["color"], blend(snapshot["previewBar"]["background"], tokens["--bg"])),
        }
        for surface in ("--bg", "--surface", "--surface-2", "--surface-3", "--surface-4"):
            ratios["signal:" + surface] = contrast(snapshot["signal"]["color"], tokens[surface])
        button = page.get_by_test_id("primary-button")
        button.hover()
        page.wait_for_timeout(220)
        hovered = button.evaluate("e => ({color: getComputedStyle(e).color, bg: getComputedStyle(e).backgroundColor})")
        ratios["primaryHover"] = contrast(hovered["color"], hovered["bg"])
        page.mouse.down()
        page.wait_for_timeout(220)
        pressed = button.evaluate("e => ({color: getComputedStyle(e).color, bg: getComputedStyle(e).backgroundColor})")
        ratios["primaryPressed"] = contrast(pressed["color"], pressed["bg"])
        page.mouse.up()
        page.mouse.move(0, 0)
        self.check("rendered text, buttons and chrome contrast >=4.5 " + label, min(ratios.values()) >= 4.5, {"ratios": ratios, "minimum": min(ratios.values()), "snapshot": snapshot, "hovered": hovered, "pressed": pressed})
        return {"case": label, "ratios": ratios, "snapshot": snapshot}

    def image_priority_and_hit_testing(self):
        page = self.new_page({"accent": "paper", "titlebarTint": True}, route="home", tauri=True)
        image = page.evaluate("() => { const canvas = document.createElement('canvas'); canvas.width=160; canvas.height=100; const ctx=canvas.getContext('2d'); ctx.fillStyle='#246c89'; ctx.fillRect(0,0,160,100); ctx.fillStyle='#e1b16c'; ctx.fillRect(80,0,80,100); return canvas.toDataURL(); }")
        self.patch(page, {"backgroundImage": image, "backgroundOpacity": 0.85, "backgroundBlur": 6})
        self.check("user image replaces shell and preview theme backgrounds", page.locator(".ni-bg > .ni-theme-artwork").count() == 0 and page.locator(".st-preview__art > .ni-theme-artwork").count() == 0 and page.locator(".ni-bg__image, .st-preview__image").count() == 2)
        self.check("user image retains original titlebar decoration", page.locator(".ni-titlebar > .ni-theme-artwork[data-motif=paper]").count() == 1)
        self.check("image style uses selected URL and live blur", page.locator(".ni-bg__image").evaluate("(e, url) => e.style.backgroundImage.includes(url) && getComputedStyle(e).filter === 'blur(6px)'", image))
        self.screenshot(page, "user-image-dark")
        self.patch(page, {"themeMode": "light"})
        self.screenshot(page, "user-image-light")
        for width in (1280, 821, 820, 760, 640):
            page.set_viewport_size({"width": width, "height": 960})
            page.wait_for_timeout(350)
            for tint in (False, True):
                self.patch(page, {"titlebarTint": tint})
                display = page.locator(".ni-titlebar > .ni-theme-artwork").evaluate("e => getComputedStyle(e).display")
                self.check("narrow titlebar decoration threshold " + str(width) + "/" + str(tint), (display == "none") == (width <= 820), display)
                hits = page.locator(".ni-titlebar button").evaluate_all("items => items.map(e => {const r=e.getBoundingClientRect(), hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2); return {label: e.getAttribute('aria-label') || e.textContent.trim(), hit: e===hit || e.contains(hit), drag: e.hasAttribute('data-tauri-drag-region'), width:r.width, x:r.x};})")
                self.check("all real nav and window buttons hit their centers " + str(width) + "/" + str(tint), len(hits) == 6 and all(hit["hit"] and not hit["drag"] for hit in hits), hits)
                drag = page.locator(".ni-brand__text").evaluate("e => {const r=e.getBoundingClientRect(), hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2); return {hit: e===hit || e.contains(hit), drag: hit?.hasAttribute('data-tauri-drag-region'), header: e.closest('header').hasAttribute('data-tauri-drag-region')};}")
                self.check("real titlebar brand remains drag-region hit " + str(width) + "/" + str(tint), drag["hit"] and drag["drag"] and drag["header"], drag)
                for label, route in (("资源", "resources"), ("更多", "more"), ("设置", "settings"), ("首页", "home")):
                    page.get_by_role("navigation", name="主导航").get_by_role("button", name=label, exact=True).click()
                    page.wait_for_function("route => window.__themeAcceptance.route === route", arg=route)
                    self.check("real TitleBar navigation handler " + str(width) + "/" + str(tint) + "/" + route, page.locator(".ni-content").get_attribute("data-route") == route)
                page.get_by_role("button", name="最小化", exact=True).click()
                page.wait_for_function("window.__themeAcceptance.calls.some(c => c.command === 'window_minimize')")
        page.set_viewport_size({"width": 820, "height": 960})
        self.screenshot(page, "narrow-820")
        page.evaluate("window.__themeAcceptance.resetMetrics()")
        page.get_by_role("button", name="关闭", exact=True).click()
        page.wait_for_function("window.__themeAcceptance.calls.some(c => c.command === 'window_close')")
        self.check("real close handler invokes backend after exit animation", page.evaluate("window.__themeAcceptance.calls.filter(c => c.command === 'window_close').length") == 1)
        page.evaluate("document.querySelector('.ni-shell').classList.remove('ni-shell--leaving')")
        self.patch(page, {"backgroundImage": None})
        self.check("clearing user image restores theme background", page.locator(".ni-bg > .ni-theme-artwork[data-motif=paper]").count() == 1 and page.locator(".ni-bg__image").count() == 0)

    def tauri_persistence(self):
        page = self.new_page({"accent": "custom", "titlebarTint": True}, tauri=True)
        nav = page.get_by_role("navigation", name="设置分区")
        nav.get_by_role("button", name="自定义", exact=True).click()
        hue = page.get_by_role("slider", name=SLIDERS["hue"])
        hue.focus()
        page.keyboard.press("ArrowRight")
        page.wait_for_function("window.__themeAcceptance.backend().customTheme.hue === 175")
        self.check("real settings_set IPC persists custom slider immediately", page.evaluate("window.__themeAcceptance.calls.some(c => c.command === 'settings_set' && JSON.parse(c.args.json).customTheme.hue === 175)"))
        page.reload(wait_until="domcontentloaded", timeout=60000)
        page.wait_for_load_state("networkidle")
        page.wait_for_function("document.documentElement.dataset.fixtureReady === 'true'")
        self.check("backend reload normalizes persisted custom theme", page.evaluate("window.__themeAcceptance.settings.customTheme.hue") == 175)
        page.get_by_role("navigation", name="设置分区").get_by_role("button", name="自定义", exact=True).click()
        page.get_by_role("button", name="重置自定义参数", exact=True).click()
        page.wait_for_function("value => JSON.stringify(window.__themeAcceptance.backend().customTheme) === JSON.stringify(value)", arg=DEFAULT_CUSTOM)
        self.check("custom reset also persists through settings_set IPC", page.evaluate("window.__themeAcceptance.backend().customTheme") == DEFAULT_CUSTOM)
        page.evaluate("window.__themeAcceptance.setFailSave(true)")
        hue = page.get_by_role("slider", name=SLIDERS["hue"])
        hue.focus()
        page.keyboard.press("ArrowRight")
        page.get_by_role("alert").wait_for()
        self.check("failed custom save is visible and does not fake backend persistence", page.get_by_role("alert").inner_text().startswith("保存设置失败") and page.evaluate("window.__themeAcceptance.backend().customTheme.hue") == 174)
        page.evaluate("window.__themeAcceptance.setFailSave(false)")
        page.keyboard.press("ArrowRight")
        page.wait_for_function("window.__themeAcceptance.backend().customTheme.hue === 176")
        self.check("next custom edit recovers failed persistence queue", page.get_by_role("alert").count() == 0)

    def runtime(self):
        context = self.browser.new_context(color_scheme="light", reduced_motion="no-preference")
        self.contexts.append(context)
        page = context.new_page()
        page.clock.install()
        page.goto(self.base + "/tests/theme-acceptance/fixtures/runtime.html")
        page.wait_for_load_state("networkidle")
        page.wait_for_function("document.documentElement.dataset.fixtureReady === 'true'")
        page.evaluate("window.__themeRuntime.start()")
        initial = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("real browser runtime starts at exact teal with one timer", initial["accent"] == "#459c93" and initial["active"] == 1, initial)
        page.clock.run_for(90000)
        midpoint = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("real runtime midpoint is exact mint at 90000ms", midpoint["accent"] == "#bcdec4" and midpoint["fired"] - initial["fired"] == 180 and midpoint["maximum"] == 1, midpoint)
        page.clock.run_for(90000)
        endpoint = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("real runtime full cycle is 180000ms and 360 ticks", endpoint["accent"] == "#459c93" and endpoint["fired"] - initial["fired"] == 360 and endpoint["active"] == 1, endpoint)
        page.clock.run_for(45000)
        page.evaluate("window.__themeRuntime.hidden(true)")
        hidden = page.evaluate("window.__themeRuntime.snapshot()")
        page.clock.run_for(180000)
        paused = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("hidden event pauses browser clock without CSS writes", paused["active"] == 0 and paused["accent"] == hidden["accent"] and paused["writes"] == hidden["writes"] and paused["dataset"]["themeMotion"] == "paused", {"before": hidden, "after": paused})
        page.evaluate("window.__themeRuntime.hidden(false)")
        page.clock.run_for(45000)
        resumed = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("visibility resumes from active elapsed time with a single timer", resumed["accent"] == "#bcdec4" and resumed["active"] == 1 and resumed["maximum"] == 1, resumed)
        page.emulate_media(reduced_motion="reduce")
        page.wait_for_function("window.__themeRuntime.snapshot().dataset.themeMotion === 'reduced'")
        reduced = page.evaluate("window.__themeRuntime.snapshot()")
        page.clock.run_for(180000)
        reduced_after = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("live reduced-motion event freezes exact start and removes timer", reduced["accent"] == "#459c93" and reduced_after["active"] == 0 and reduced_after["fired"] == reduced["fired"], reduced_after)
        page.evaluate("window.__themeRuntime.start({accent:'custom',themeMode:'system',titlebarTint:true})")
        self.check("system light mode resolves in real browser", page.evaluate("window.__themeRuntime.snapshot().dataset.theme") == "light")
        page.emulate_media(color_scheme="dark")
        page.wait_for_function("window.__themeRuntime.snapshot().dataset.theme === 'dark'")
        self.check("system theme change repaints without a timer", page.evaluate("window.__themeRuntime.snapshot().active") == 0)
        page.evaluate("window.__themeRuntime.stop()")
        stopped = page.evaluate("window.__themeRuntime.snapshot()")
        self.check("runtime cleanup removes owned CSS and dataset", stopped["active"] == 0 and stopped["properties"] == [] and stopped["dataset"] == {}, stopped)
        page.emulate_media(reduced_motion="no-preference")
        for accent in ORDER:
            page.evaluate("accent => window.__themeRuntime.start({accent})", accent)
            self.check("real runtime static/timer count " + accent, page.evaluate("window.__themeRuntime.snapshot().active") == (1 if accent == "fortyTwo" else 0))
            page.evaluate("window.__themeRuntime.stop()")
        app = self.new_page({"accent": "nickel"}, route="home")
        app.clock.install()
        app.evaluate(TRACKER)
        self.patch(app, {"accent": "fortyTwo"})
        app.clock.run_for(1000)
        app.evaluate("window.__themeAcceptance.resetMetrics()")
        before = app.evaluate("({commits:window.__themeAcceptance.commits,accent:document.documentElement.style.getPropertyValue('--accent')})")
        app.clock.run_for(90000)
        after = app.evaluate("({commits:window.__themeAcceptance.commits,accent:document.documentElement.style.getPropertyValue('--accent')})")
        self.check("dynamic CSS color update causes no React commits", after["commits"] == before["commits"] == 0 and after["accent"] != before["accent"], {"before": before, "after": after})
        self.check("StrictMode provider has only one active theme timer", app.evaluate("window.__themeTimers.snapshot().active") == 1, app.evaluate("window.__themeTimers.snapshot()"))
        app.evaluate("window.__themeAcceptance.unmount()")
        self.check("provider unmount clears timer and runtime CSS", app.evaluate("window.__themeTimers.snapshot().active") == 0 and app.evaluate("document.documentElement.style.getPropertyValue('--accent')") == "")

    def finish(self):
        self.check("browser has no uncaught exceptions or error console entries", not self.errors, self.errors)
        report = {
            "startedAt": self.started,
            "finishedAt": datetime.now(timezone.utc).isoformat(),
            "baseUrl": self.base,
            "browserVersion": self.browser.version,
            "checks": self.results,
            "passed": sum(result["passed"] for result in self.results),
            "failed": sum(not result["passed"] for result in self.results),
            "errors": self.errors,
            "screenshots": self.screenshots,
        }
        path = self.evidence / "browser-evidence.json"
        path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        for context in self.contexts:
            context.close()
        print(json.dumps({"passed": report["passed"], "failed": report["failed"], "evidence": str(path.resolve())}), flush=True)
        return 0 if report["failed"] == 0 else 1


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:5191")
    parser.add_argument("--evidence-dir", type=Path, default=Path(__file__).resolve().parents[1] / "evidence" / "browser")
    args = parser.parse_args()
    with sync_playwright() as playwright:
        launch_errors = []
        browser = None
        options = [{"channel": os.environ.get("THEME_BROWSER_CHANNEL", "msedge")}]
        cached = Path(os.environ.get("LOCALAPPDATA", "")) / "ms-playwright" / "chromium-1234" / "chrome-win64" / "chrome.exe"
        if os.environ.get("THEME_BROWSER_EXECUTABLE"):
            options.insert(0, {"executable_path": os.environ["THEME_BROWSER_EXECUTABLE"]})
        if cached.is_file():
            options.append({"executable_path": str(cached)})
        options.append({})
        for option in options:
            try:
                browser = playwright.chromium.launch(headless=True, **option)
                break
            except Exception as error:
                launch_errors.append(str(error))
        if browser is None:
            raise RuntimeError("Unable to start headless browser: " + "\n".join(launch_errors))
        acceptance = Acceptance(browser, args.base_url, args.evidence_dir)
        for name, callback in [("palette", acceptance.palette), ("custom", acceptance.custom), ("visual and contrast", acceptance.visual_and_contrast), ("image priority and hit testing", acceptance.image_priority_and_hit_testing), ("Tauri persistence", acceptance.tauri_persistence), ("runtime", acceptance.runtime)]:
            acceptance.run_section(name, callback)
        code = acceptance.finish()
        browser.close()
        return code


if __name__ == "__main__":
    sys.exit(main())
