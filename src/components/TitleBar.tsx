import { type ComponentType } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAppNav, type Route } from "../lib/nav";
import { useSlidePill } from "../lib/useSlidePill";
import { isTauri, useSettings } from "../lib/settings";
import { ThemeArtwork } from "./ui/ThemeArtwork";
import { PRODUCT_NAME } from "../lib/brand";
import { Tooltip } from "./ui";
import {
  IconClose,
  IconHome,
  IconMinus,
  IconResources,
  IconSettings,
  IconVersions,
  type IconProps,
} from "./ui/icons";

interface NavItem {
  route: Route;
  label: string;
  Icon: ComponentType<IconProps>;
}

/**
 * 顶栏导航项 —— 与 `nav.ts` 的 `TAB_ROUTES` 一一对应（顺序一致）。
 *
 * 红线圈定：顶栏只有这四项，**不得新增任何 tab**；顶栏之外的页面一律是
 * 「无 tab 归属的独立页 + 页面内返回」，通过首页等入口进入。
 * 「更多」沿用原「版本」的图标 IconVersions（层叠方块），代表其余入口。
 */
const NAV_ITEMS: NavItem[] = [
  { route: "home", label: "首页", Icon: IconHome },
  { route: "resources", label: "资源", Icon: IconResources },
  { route: "settings", label: "设置", Icon: IconSettings },
  { route: "more", label: "更多", Icon: IconVersions },
];

/** Duration of the shell exit animation before the window is actually closed. */
const EXIT_MS = 300;

async function runWindowCommand(
  command: "window_minimize" | "window_close"
): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke(command);
  } catch {
    /* not running inside a Tauri host — ignore */
  }
}

/** Play the shell exit animation, then close the window (Tauri only). */
async function closeWindow(): Promise<void> {
  if (!isTauri()) return;
  const shell = document.querySelector<HTMLElement>(".nw-shell");
  if (shell && !shell.classList.contains("nw-shell--leaving")) {
    shell.classList.add("nw-shell--leaving");
    await new Promise((resolve) => window.setTimeout(resolve, EXIT_MS));
  }
  await runWindowCommand("window_close");
}

export function TitleBar() {
  const { route, setRoute } = useAppNav();
  const { settings } = useSettings();
  const activeIndex = NAV_ITEMS.findIndex((item) => item.route === route);
  const { navRef, registerTab, pill } = useSlidePill<
    HTMLElement,
    HTMLButtonElement
  >(activeIndex);

  return (
    <header
      className="nw-titlebar"
      data-tauri-drag-region
      data-tinted={settings.titlebarTint}
    >
      <ThemeArtwork
        accent={settings.accent}
        placement="titlebar"
        decoration={settings.customTheme.decoration}
      />
      <div className="nw-titlebar__left" data-tauri-drag-region>
        <span className="nw-brand">
          <span className="nw-brand__text" data-tauri-drag-region>
            {PRODUCT_NAME}
          </span>
        </span>
      </div>

      <nav className="nw-titlebar__center" aria-label="主导航" ref={navRef}>
        <span
          className="nw-navtab__pill"
          data-ready={pill.ready}
          style={{ transform: `translateX(${pill.left}px)`, width: pill.width }}
          aria-hidden="true"
        />
        {NAV_ITEMS.map(({ route: r, label, Icon }, i) => (
          <button
            key={r}
            ref={registerTab(i)}
            type="button"
            className="nw-navtab"
            data-active={route === r}
            aria-current={route === r ? "page" : undefined}
            onClick={() => setRoute(r)}
          >
            <Icon />
            <span>{label}</span>
          </button>
        ))}
      </nav>

      <div className="nw-titlebar__right">
        <Tooltip label="最小化">
          <button
            type="button"
            className="nw-winbtn"
            aria-label="最小化"
            onClick={() => void runWindowCommand("window_minimize")}
          >
            <IconMinus />
          </button>
        </Tooltip>
        <Tooltip label="关闭">
          <button
            type="button"
            className="nw-winbtn nw-winbtn--close"
            aria-label="关闭"
            onClick={() => void closeWindow()}
          >
            <IconClose />
          </button>
        </Tooltip>
      </div>
    </header>
  );
}
