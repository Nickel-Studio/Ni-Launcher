import { useState } from "react";
import { useSettings } from "../lib/settings";
import { LoadingCard, PagerLayout, type PagerItem } from "../components/ui";
import { AppearancePanel } from "../components/settings/AppearancePanel";
import { AccountPanel } from "../components/settings/AccountPanel";
import { DownloadPanel } from "../components/settings/DownloadPanel";
import { GamePanel } from "../components/settings/GamePanel";
import { IconPlay } from "../components/ui";
import {
  DownloadIcon,
  PaletteIcon,
  UserShieldIcon,
} from "../components/settings/icons";
import "../components/settings/settings.css";

/**
 * 设置分区。侧边栏是**页面切换器**（点击 = 右侧整页替换），不是滚动锚点导航：
 * 因此这里没有滚动监听、没有锚点 id 查找，也没有「滚动到某分区」的逻辑。
 */
const SECTIONS: PagerItem[] = [
  { id: "appearance", label: "外观", icon: <PaletteIcon /> },
  { id: "account", label: "账号与隐私", icon: <UserShieldIcon /> },
  { id: "download", label: "下载", icon: <DownloadIcon /> },
  { id: "game", label: "游戏", icon: <IconPlay /> },
];

/** 设置页：左侧分区切换器 + 右侧各分区独立页。 */
export default function Settings() {
  const { settings, update, ready, saveError } = useSettings();
  const [active, setActive] = useState<string>(SECTIONS[0].id);
  if (!ready)
    return saveError ? (
      <p className="st-save-error" role="alert">
        {saveError}
      </p>
    ) : (
      <LoadingCard className="nw-loading--viewport" label="正在载入设置..." />
    );
  return (
    <PagerLayout
      className="nw-pager--fill"
      items={SECTIONS}
      active={active}
      onSelect={setActive}
      navLabel="设置分区"
      navTitle="设置"
    >
      {saveError ? (
        <p className="st-save-error" role="alert">
          {saveError}
        </p>
      ) : null}
      {active === "appearance" ? (
        <AppearancePanel settings={settings} update={update} />
      ) : active === "account" ? (
        <AccountPanel settings={settings} update={update} />
      ) : active === "download" ? (
        <DownloadPanel settings={settings} update={update} />
      ) : (
        <GamePanel />
      )}
    </PagerLayout>
  );
}
