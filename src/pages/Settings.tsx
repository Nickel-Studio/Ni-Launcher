import { useEffect, useMemo, useState } from "react";
import { useSettings } from "../lib/settings";
import { LoadingCard, PagerLayout, type PagerItem } from "../components/ui";
import { AppearancePanel } from "../components/settings/AppearancePanel";
import { CustomThemePanel } from "../components/settings/CustomThemePanel";
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

const SECTIONS: PagerItem[] = [
  { id: "appearance", label: "外观", icon: <PaletteIcon /> },
  { id: "account", label: "账号与隐私", icon: <UserShieldIcon /> },
  { id: "download", label: "下载", icon: <DownloadIcon /> },
  { id: "game", label: "游戏", icon: <IconPlay /> },
];
const CUSTOM_SECTION: PagerItem = {
  id: "custom-theme",
  label: "自定义",
  icon: <PaletteIcon />,
};

/** Each navigation item replaces the panel rather than scrolling to an anchor. */
export default function Settings() {
  const { settings, update, ready, saveError } = useSettings();
  const [active, setActive] = useState<string>(SECTIONS[0].id);
  const customSelected = settings.accent === "custom";
  const sections = useMemo(
    () =>
      customSelected
        ? [SECTIONS[0], CUSTOM_SECTION, ...SECTIONS.slice(1)]
        : SECTIONS,
    [customSelected]
  );
  const selected =
    active === "custom-theme" && !customSelected ? "appearance" : active;
  useEffect(() => {
    if (!customSelected && active === "custom-theme") setActive("appearance");
  }, [customSelected, active]);
  if (!ready)
    return saveError ? (
      <p className="st-save-error" role="alert">
        {saveError}
      </p>
    ) : (
      <LoadingCard className="ni-loading--viewport" label="正在载入设置..." />
    );
  return (
    <PagerLayout
      className="ni-pager--fill"
      items={sections}
      active={selected}
      onSelect={setActive}
      navLabel="设置分区"
      navTitle="设置"
    >
      {saveError ? (
        <p className="st-save-error" role="alert">
          {saveError}
        </p>
      ) : null}
      {selected === "appearance" ? (
        <AppearancePanel
          settings={settings}
          update={update}
          onOpenCustom={() => setActive("custom-theme")}
        />
      ) : selected === "custom-theme" && customSelected ? (
        <CustomThemePanel settings={settings} update={update} />
      ) : selected === "account" ? (
        <AccountPanel settings={settings} update={update} />
      ) : selected === "download" ? (
        <DownloadPanel settings={settings} update={update} />
      ) : (
        <GamePanel />
      )}
    </PagerLayout>
  );
}
