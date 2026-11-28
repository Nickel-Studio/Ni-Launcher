import { createRoot } from "react-dom/client";
import { useEffect } from "react";
import { SettingsProvider } from "../../src/lib/settings";
import { setLaunchInstance } from "../../src/lib/instances";
import { LauncherProvider } from "../../src/components/launch/useLauncher";
import { Home } from "../../src/pages/Home";
import { MicrosoftAccount } from "../../src/components/account/MicrosoftAccount";
import { ToastHost } from "../../src/components/ui";
import { dismissToast, useToasts } from "../../src/lib/toast";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
const bridge = window as any;
const calls: any[] = [];
let settings: any = {
  accountType: "microsoft",
  skinAuthMethod: "oauth2",
  littleskinServer: "https://a.example",
};
let accounts = [
  {
    id: "msa",
    type: "microsoft",
    name: "Original Microsoft",
    uuid: "11111111111111111111111111111111",
  },
  {
    id: "alpha",
    type: "littleskin",
    name: "Alpha Player",
    server: "https://a.example",
    authMethod: "oauth2",
    email: "oauth@example.test",
    uuid: "22222222222222222222222222222222",
  },
  {
    id: "beta",
    type: "littleskin",
    name: "Beta Player",
    server: "https://b.example",
    authMethod: "classic",
    email: "classic@example.test",
    uuid: "33333333333333333333333333333333",
  },
];
const fixtureMode = new URLSearchParams(location.search).get("login");
if (fixtureMode) {
  accounts = [];
  settings.accountType =
    fixtureMode === "microsoft" ? "microsoft" : "littleskin";
  settings.skinAuthMethod = fixtureMode === "classic" ? "classic" : "oauth2";
  settings.littleskinServer = "https://empty.example";
}
let active = "msa";
let releaseBegin: ((id: string) => void) | null = null;
const select = (versionId: string) =>
  setLaunchInstance({
    root: "C:/OAuthFixture",
    versionId,
    loader: "fabric",
    icon: "grass",
  });
select("normal");
localStorage.setItem("nickel.accounts", JSON.stringify(accounts));
localStorage.setItem("nickel.accounts.active", active);
bridge.__home = {
  calls,
  select,
  active: () => active,
  settings: () => settings,
  releaseBegin: () => releaseBegin?.("oauth_fixture"),
  dismissToast,
  oauthMode: "pending",
  msaMode: "pending",
  notifications: [],
  lastToastId: 0,
};
function ToastProbe() {
  const toasts = useToasts();
  useEffect(() => {
    for (const toast of toasts) {
      if (toast.id <= bridge.__home.lastToastId) continue;
      bridge.__home.lastToastId = toast.id;
      bridge.__home.notifications.push(toast);
    }
  }, [toasts]);
  return null;
}
bridge.__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: any = {}) => {
    calls.push({ command, args });
    if (command === "settings_get") return JSON.stringify(settings);
    if (command === "settings_set") {
      settings = JSON.parse(args.json);
      return;
    }
    if (command === "accounts_list") return accounts;
    if (command === "accounts_set_active") {
      if (bridge.__home.activateError && args.id)
        throw new Error("Fixture activation failure");
      active = args.id;
      return;
    }
    if (command === "accounts_upsert") {
      if (bridge.__home.pauseSave) {
        await new Promise<void>((resolve) => {
          bridge.__home.releaseSave = resolve;
        });
      }
      if (bridge.__home.writeError) throw new Error("Fixture write failure");
      accounts = accounts
        .filter((a) => a.id !== args.account.id)
        .concat(args.account);
      return accounts;
    }
    if (command === "skin_auth_context") {
      const exclusive = args.versionId !== "normal";
      const server = exclusive
        ? `https://${args.versionId}.example`
        : settings.littleskinServer;
      return {
        exclusive,
        method: settings.skinAuthMethod,
        site: {
          server,
          api: `${server}/api/yggdrasil`,
          name: exclusive
            ? `${args.versionId.toUpperCase()} Skin`
            : "Alpha Skin",
          homepage: server,
          register: "",
        },
        root: args.root,
        versionId: args.versionId,
      };
    }
    if (command === "skin_site_describe")
      return {
        server: args.server,
        api: `${args.server}/api/yggdrasil`,
        name: args.server.includes("a.example") ? "Alpha Skin" : "Beta Skin",
        homepage: args.server,
        register: "",
      };
    if (command === "msa_config_get")
      return {
        clientId: "fixture-msa",
        authority: "consumers",
        redirectUri: "fixture",
      };
    if (command === "msa_device_start") {
      if (bridge.__home.msaMode === "start-error")
        throw new Error("无法获取设备码");
      if (bridge.__home.msaMode === "start-pending") {
        await new Promise<void>((resolve) => {
          bridge.__home.releaseMsaStart = resolve;
        });
      }
      return {
        userCode: "FIXTURE-CODE",
        deviceCode: "fixture-device-code",
        verificationUri: "https://example.test/device",
        interval: 1,
        expiresIn: bridge.__home.msaMode === "expiry" ? 0.05 : 900,
      };
    }
    if (command === "msa_device_poll") {
      return new Promise((resolve, reject) => {
        bridge.__home.resolveMsa = resolve;
        bridge.__home.rejectMsa = reject;
      });
    }
    if (command === "plugin:opener|open_url") {
      if (bridge.__home.openError) throw new Error("Fixture browser failure");
      return;
    }
    if (command === "yggdrasil_login") {
      return new Promise((resolve, reject) => {
        bridge.__home.resolveClassic = resolve;
        bridge.__home.rejectClassic = reject;
      });
    }
    if (command === "skin_oauth_begin") {
      if (bridge.__home.oauthMode === "begin-error")
        throw new Error("无法打开授权页面");
      if (bridge.__home.oauthMode !== "pending") return "oauth_fixture";
      return new Promise<string>((resolve) => {
        releaseBegin = resolve;
      });
    }
    if (command === "skin_oauth_cancel" || command === "yggdrasil_cancel")
      return;
    if (command === "skin_oauth_finish") {
      if (bridge.__home.oauthMode === "pending")
        throw new Error("Cancelled begin must not finish");
      return new Promise((resolve, reject) => {
        bridge.__home.finish = resolve;
        bridge.__home.rejectFinish = reject;
      });
    }
    if (command === "yggdrasil_select_profile") {
      if (bridge.__home.selectError) throw new Error("选角失败，请重试");
      if (bridge.__home.pauseSelect) {
        await new Promise<void>((resolve) => {
          bridge.__home.releaseSelect = resolve;
        });
      }
      return {
        id: "selected",
        type: "littleskin",
        name: "Selected Player",
        uuid: args.profileId,
        server: "https://empty.example",
      };
    }
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return;
    if (command.includes("skin") || command.includes("avatar")) return null;
    throw new Error(`Unexpected command ${command}`);
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
  convertFileSrc: (s: string) => s,
};
bridge.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
createRoot(document.getElementById("root")!).render(
  <SettingsProvider>
    <LauncherProvider>
      <Home />
      <div id="microsoft-reference" style={{ width: 420, padding: 20 }}>
        <MicrosoftAccount
          identity={{ name: "", email: "", loggedIn: false }}
          onAccount={() => {}}
          onLogout={() => {}}
        />
      </div>
      <ToastHost />
      <ToastProbe />
    </LauncherProvider>
  </SettingsProvider>
);
