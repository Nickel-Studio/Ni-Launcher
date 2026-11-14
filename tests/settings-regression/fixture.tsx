/** 真实 GamePanel + 可控 IPC；运行 pnpm dev 后访问 /tests/settings-regression/regression.html。 */
import { createRoot } from "react-dom/client";
import { SettingsProvider } from "../../src/lib/settings";
import { GamePanel } from "../../src/components/settings/GamePanel";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";

type RuntimeState = {
  totalMemoryMb: number;
  availableMemoryMb: number;
  suggestedMemoryMb: number;
  fail: boolean;
};

const runtimeState: RuntimeState = {
  totalMemoryMb: 16384,
  availableMemoryMb: 10000,
  suggestedMemoryMb: 4096,
  fail: false,
};
const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
let callbackId = 0;
let pollCount = 0;
let savedSettings: Record<string, unknown> = {
  gameDir: "",
  javaPath: "",
  versionIsolation: true,
  windowTitle: "",
  customInfo: "",
  launcherVisibility: "keep",
  processPriority: "normal",
  windowMode: "default",
  windowWidth: 854,
  windowHeight: 480,
  memoryMode: "custom",
  memoryMb: 8192,
  memoryManagement: "g1-tuned",
  memoryOptimize: false,
  highPerformanceGpu: false,
  disableWrapper: false,
  disableLwjglAgent: false,
  offlineSkin: "random",
  offlineSkinName: "",
  offlineSkinPath: "",
  offlineSkinSlim: false,
  jvmArgs: "",
  gameArgs: "",
  preLaunchCommand: "",
};

const runtimeInfo = () => ({
  totalMemoryMb: runtimeState.totalMemoryMb,
  availableMemoryMb: runtimeState.availableMemoryMb,
  suggestedMemoryMb: runtimeState.suggestedMemoryMb,
  capabilities: {
    windowTitle: true,
    processPriority: true,
    memoryOptimize: true,
    offlineSkin: true,
    launchWrapper: true,
    lwjglAgent: true,
    highPerformanceGpu: true,
  },
});

const mock = {
  invoke: async (command: string, args: Record<string, unknown> = {}) => {
    calls.push({ command, args });
    if (command === "settings_get") {
      return JSON.stringify({ themeMode: "dark", accent: "mono" });
    }
    if (command === "settings_set") return null;
    if (command === "game_settings_get") return JSON.stringify(savedSettings);
    if (command === "game_settings_set") {
      savedSettings = JSON.parse(String(args.json));
      return null;
    }
    if (command === "game_runtime_info") {
      pollCount += 1;
      if (runtimeState.fail) throw new Error("模拟读取失败");
      return runtimeInfo();
    }
    if (command === "resolved_memory_plan") {
      return { ...runtimeInfo(), modCount: 0 };
    }
    if (command === "java_detect") return [];
    if (command === "plugin:event|listen") return ++callbackId;
    if (command === "plugin:event|unlisten") return null;
    if (command === "plugin:dialog|message") return "Ok";
    throw new Error(`未模拟命令：${command}`);
  },
  transformCallback: () => ++callbackId,
  unregisterCallback: () => {},
  convertFileSrc: (value: string) => value,
};

Object.assign(window, {
  __TAURI_INTERNALS__: mock,
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
  __settingsRegression: {
    calls,
    runtimeState,
    get pollCount() {
      return pollCount;
    },
    setAvailable(value: number) {
      runtimeState.availableMemoryMb = value;
    },
    setFail(value: boolean) {
      runtimeState.fail = value;
    },
    resetCalls() {
      calls.length = 0;
    },
    getSettings() {
      return savedSettings;
    },
  },
});

const root = createRoot(document.getElementById("root")!);
root.render(
  <SettingsProvider>
    <div className="fixture">
      <GamePanel />
    </div>
  </SettingsProvider>
);
