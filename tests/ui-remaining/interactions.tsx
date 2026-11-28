import { createRoot } from "react-dom/client";
import { SettingsProvider } from "../../src/lib/settings";
import { GamePanel } from "../../src/components/settings/GamePanel";
import { SettingsPanel } from "../../src/components/version/vsettings/panels";
import { Tooltip, Button, IconButton } from "../../src/components/ui";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";
import "../../src/pages/VersionSettings.css";

const bridge = window as any;
const javas = [{ path: "C:/Fixture/Java21/bin/javaw.exe", version: "21.0.4", vendor: "Fixture Vendor", major: 21, bits: 64 }];
let game = { javaPath: "" };
let version = { javaPath: "" };
bridge.__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: any = {}) => {
    if (command === "settings_get") return "{}";
    if (command === "game_settings_get") return JSON.stringify(game);
    if (command === "version_settings_get") return JSON.stringify(version);
    if (command === "game_settings_set") { game = JSON.parse(args.json); return; }
    if (command === "version_settings_set") { version = JSON.parse(args.json); return; }
    if (command === "java_detect") return javas;
    if (command === "version_detail") return null;
    if (command === "game_runtime_info" || command === "resolved_memory_plan") {
      return { totalMemoryMb: 16384, availableMemoryMb: 8192, suggestedMemoryMb: 4096, modCount: 0, capabilities: {} };
    }
    if (command === "skin_site_describe") return { server: args.server, name: "LittleSkin", api: "", homepage: "", register: "" };
    if (command === "settings_set" || command === "plugin:event|listen" || command === "plugin:event|unlisten") return 1;
    throw new Error(`Unexpected fixture command: ${command}`);
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
};
bridge.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
const scenario = new URLSearchParams(location.search).get("scenario");
createRoot(document.getElementById("root")!).render(
  <SettingsProvider>
    <main style={{ padding: 32 }}>
      {scenario === "global" ? <GamePanel /> : scenario === "instance" ? (
        <SettingsPanel root="C:/Fixture" versionId="v" javas={javas} />
      ) : (
        <div style={{ display: "grid", gap: 30, width: 420 }}>
          <Tooltip label="Parent help"><div id="parent" style={{ padding: 40 }}>
            <Tooltip label="Child help"><input id="nested" aria-label="Nested input" title="Native duplicate" /></Tooltip>
          </div></Tooltip>
          <Tooltip label="Input help"><input id="input" aria-label="Pointer input" /></Tooltip>
          <Tooltip label="Outer help"><IconButton label="Icon help"><span>i</span></IconButton></Tooltip>
          <Button disabled title="Disabled help">Disabled control</Button>
        </div>
      )}
    </main>
  </SettingsProvider>
);
