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
 * 独立详情页不占顶栏高亮，通过页面内返回恢复来源页。
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
  const shell = document.querySelector<HTMLElement>(".ni-shell");
  if (shell && !shell.classList.contains("ni-shell--leaving")) {
    shell.classList.add("ni-shell--leaving");
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
      className="ni-titlebar"
      data-tauri-drag-region
      data-tinted={settings.titlebarTint}
    >
      <ThemeArtwork
        accent={settings.accent}
        placement="titlebar"
        decoration={settings.customTheme.decoration}
      />
      <div className="ni-titlebar__left" data-tauri-drag-region>
        <span className="ni-brand">
          <span className="ni-brand__text" data-tauri-drag-region>
            {PRODUCT_NAME}
          </span>
        </span>
      </div>

      <nav className="ni-titlebar__center" aria-label="主导航" ref={navRef}>
        <span
          className="ni-navtab__pill"
          data-ready={pill.ready}
          style={{ transform: `translateX(${pill.left}px)`, width: pill.width }}
          aria-hidden="true"
        />
        {NAV_ITEMS.map(({ route: r, label, Icon }, i) => (
          <button
            key={r}
            ref={registerTab(i)}
            type="button"
            className="ni-navtab"
            data-active={route === r}
            aria-current={route === r ? "page" : undefined}
            onClick={() => setRoute(r)}
          >
            <Icon />
            <span>{label}</span>
          </button>
        ))}
      </nav>

      <div className="ni-titlebar__right">
        <Tooltip label="最小化">
          <button
            type="button"
            className="ni-winbtn"
            aria-label="最小化"
            onClick={() => void runWindowCommand("window_minimize")}
          >
            <IconMinus />
          </button>
        </Tooltip>
        <Tooltip label="关闭">
          <button
            type="button"
            className="ni-winbtn ni-winbtn--close"
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
