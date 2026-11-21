import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/pages/VersionSettings.css";
import { SettingsProvider } from "../../src/lib/settings";
import { ExportPanel } from "../../src/components/version/vsettings/ExportPanel";
import type {
  ExportConfig,
  ExportFile,
  ExportGroup,
} from "../../src/components/version/vsettings/export";

function directory(
  sourcePath: string,
  group: ExportGroup,
  selected = true,
  displayName = sourcePath.split("/").pop()!
): ExportFile {
  return {
    path: `@dir/${group}/${sourcePath}`,
    sourcePath,
    group,
    selected,
    displayName,
    kind: "directory",
    sizeBytes: null,
  };
}
function file(
  path: string,
  group: ExportGroup,
  selected = true,
  displayName = path.split("/").pop()!
): ExportFile {
  return {
    path,
    sourcePath: path,
    group,
    selected,
    displayName,
    kind: "file",
    sizeBytes: 42,
  };
}
const preview = {
  name: "Export Regression Pack",
  minecraftVersion: "1.20.1",
  loader: { kind: "fabric", version: "0.16.0" },
  launcher: {
    available: true,
    detail: "Fixture launcher",
    os: "windows",
    arch: "x86_64",
  },
  instanceSettings: ["Window size"],
  instanceSettingsOmitted: [],
  instanceSettingsError: null,
  files: [
    file("options.txt", "gameOptions"),
    file("hotbar.nbt", "personal", false),
    file("servers.dat", "servers", false),
    file("optionsshaders.txt", "shaderOptions"),
    directory("config", "config", true, "Mod configuration"),
    directory("defaultconfigs", "config", true, "Default world configuration"),
    directory("journeymap", "config", true, "JourneyMap configuration"),
    directory("journeymap", "maps", false, "JourneyMap personal maps"),
    directory("config/jei/world", "jeiPersonal", false, "JEI personal data"),
    file("emi.json", "emiPersonal", false),
    file("patchouli_data.json", "patchouliPersonal", false),
    directory("screenshots", "screenshots", false, "Screenshots"),
    directory("schematics", "schematics", false, "Schematics"),
    directory("replay_recordings", "replays", false, "Replay recordings"),
    directory("replay_videos", "replays", false, "Replay videos"),
    directory("saves/Selected", "saves", false, "Selected world"),
    directory("saves/Other", "saves", false, "Other world"),
    directory("tacz", "tacz", true, "TaCZ packs"),
    directory("config/tacz/custom", "tacz", true, "TaCZ custom packs"),
    directory("immersive_paintings", "paintings", true, "Uploaded paintings"),
    directory("kubejs", "packData", true, "KubeJS"),
    directory("patchouli_books", "packData", true, "Patchouli books"),
    directory("shaderpacks/Forest", "shaderpacks", true, "Forest shaders"),
    file("mods/disabled-fixture.jar.disabled", "disabledMods", false),
    directory("mods/nested", "mods", true, "Nested enabled mods"),
    directory("mods/nested", "disabledMods", false, "Nested disabled mods"),
    ...Array.from({ length: 140 }, (_, index) =>
      file(`mods/fixture-${index}.jar`, "mods")
    ),
    ...Array.from({ length: 100 }, (_, index) =>
      directory(
        `resourcepacks/Pack-${index}`,
        "resourcepacks",
        true,
        index === 0 ? "Emerald Workshop" : `Resource pack ${index}`
      )
    ),
    file("resourcepacks/Archive.zip", "resourcepacks", true, "Archive pack"),
  ],
};

const harness = {
  preview,
  calls: [] as { command: string; args: Record<string, unknown> }[],
  loadedConfig: null as ExportConfig | null,
  savedConfigs: [] as ExportConfig[],
};
let callbackId = 0;
Object.assign(window, {
  __exportRegression: harness,
  __TAURI_INTERNALS__: {
    async invoke(command: string, args: Record<string, unknown> = {}) {
      harness.calls.push({ command, args });
      if (command === "settings_get") return "{}";
      if (command === "settings_set" || command === "plugin:event|unlisten")
        return null;
      if (command === "plugin:event|listen") return ++callbackId;
      if (command === "pack_export_inspect") return structuredClone(preview);
      if (command === "pack_export_config_load")
        return structuredClone(harness.loadedConfig);
      if (command === "pack_export_config_save") {
        harness.savedConfigs.push(structuredClone(args.config as ExportConfig));
        return "fixture-export-config.json";
      }
      throw new Error(`Unexpected fixture command: ${command}`);
    },
    transformCallback: () => ++callbackId,
    unregisterCallback: () => {},
    convertFileSrc: (value: string) => value,
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <SettingsProvider>
      <main style={{ maxWidth: 1000, padding: 24, margin: "0 auto" }}>
        <ExportPanel root="fixture-instance" versionId="fixture-version" />
      </main>
    </SettingsProvider>
  </StrictMode>
);
