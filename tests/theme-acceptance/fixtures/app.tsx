import { Profiler, StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { TitleBar } from "../../../src/components/TitleBar";
import { BackgroundLayer } from "../../../src/components/ui/BackgroundLayer";
import { ThemeArtwork } from "../../../src/components/ui/ThemeArtwork";
import { ThemePreview } from "../../../src/components/settings/ThemePreview";
import {
  Button,
  Card,
  SegmentedControl,
  Slider,
  Switch,
} from "../../../src/components/ui";
import Settings from "../../../src/pages/Settings";
import More from "../../../src/pages/More";
import {
  DEFAULT_SETTINGS,
  normalizeSettings,
  SettingsProvider,
  useSettings,
  type AppSettings,
} from "../../../src/lib/settings";
import {
  getRoute,
  setRoute,
  useAppNav,
  type Route,
} from "../../../src/lib/nav";
import * as theme from "../../../src/lib/theme";
import "../../../src/styles/tokens.css";
import "../../../src/styles/global.css";
import "../../../src/styles/ui.css";
import "../../../src/styles/shell.css";
import "./fixture.css";

const query = new URLSearchParams(location.search);
const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
let callbackId = 0;
let latest: AppSettings = normalizeSettings(DEFAULT_SETTINGS);
let updateSettings: ((patch: Partial<AppSettings>) => void) | undefined;
let commits = 0;
let failSave = false;
const settingsKey = "nowhere.settings";
const backendKey = "theme.acceptance.backend";

if (query.get("host") === "tauri") {
  Object.assign(window, {
    __TAURI_INTERNALS__: {
      invoke: async (command: string, args: Record<string, unknown> = {}) => {
        calls.push({ command, args });
        if (command === "settings_get")
          return (
            localStorage.getItem(backendKey) ??
            localStorage.getItem(settingsKey) ??
            "{}"
          );
        if (command === "settings_set") {
          if (failSave) throw new Error("fixture save failure");
          localStorage.setItem(backendKey, String(args.json));
          return null;
        }
        if (command === "window_minimize" || command === "window_close")
          return null;
        if (command === "plugin:event|listen") return ++callbackId;
        if (command === "plugin:event|unlisten") return null;
        throw new Error(`Unsupported fixture command: ${command}`);
      },
      transformCallback: () => ++callbackId,
      unregisterCallback: () => {},
      convertFileSrc: (value: string) => value,
    },
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
  });
}

const api = {
  theme,
  calls,
  get settings() {
    return latest;
  },
  get commits() {
    return commits;
  },
  get route() {
    return getRoute();
  },
  update(patch: Partial<AppSettings>) {
    if (!updateSettings) throw new Error("Settings bridge is not ready");
    updateSettings(patch);
  },
  saved() {
    return JSON.parse(localStorage.getItem(settingsKey) ?? "{}");
  },
  backend() {
    return JSON.parse(localStorage.getItem(backendKey) ?? "{}");
  },
  resetMetrics() {
    commits = 0;
    calls.length = 0;
  },
  setFailSave(value: boolean) {
    failSave = value;
  },
  navigate(route: Route) {
    setRoute(route);
  },
  unmount() {
    root.unmount();
  },
};
Object.assign(window, { __themeAcceptance: api });

function Specimen() {
  const { settings } = useSettings();
  return (
    <div className="ta-specimen">
      <div className="ta-art-stage" data-testid="art-stage">
        <ThemeArtwork
          accent={settings.accent}
          decoration={settings.customTheme.decoration}
        />
        <div className="ta-art-stage__text">
          <span className="nw-eyebrow">NICKEL / COLOR STUDY</span>
          <h1>Theme acceptance</h1>
          <p>Neutral reading surfaces and original edge geometry.</p>
        </div>
      </div>
      <Card title="Readable controls" subtitle="Actual shared UI components">
        <div className="ta-controls">
          <Button variant="primary" data-testid="primary-button">
            Primary action
          </Button>
          <Button variant="secondary">Secondary action</Button>
          <Switch
            checked={true}
            onChange={() => {}}
            aria-label="Sample switch"
          />
          <SegmentedControl
            value="one"
            onChange={() => {}}
            options={[
              { value: "one", label: "Selected" },
              { value: "two", label: "Other" },
            ]}
          />
          <Slider value={60} onChange={() => {}} aria-label="Sample slider" />
        </div>
        <p className="ta-signal" data-testid="signal-text">
          Readable theme signal
        </p>
        <p className="ta-body-text">
          Body text stays neutral in both appearance modes.
        </p>
      </Card>
      <ThemePreview />
    </div>
  );
}

function Shell() {
  const { settings, update, ready } = useSettings();
  const { route } = useAppNav();
  latest = settings;
  updateSettings = update;
  useEffect(() => {
    document.documentElement.dataset.fixtureReady = String(ready);
  }, [ready]);
  return (
    <div className="nw-shell">
      <BackgroundLayer />
      <TitleBar />
      <div className="nw-body">
        <main className="nw-content" data-route={route}>
          {route === "settings" ? (
            <Settings />
          ) : route === "more" ? (
            <More />
          ) : (
            <Specimen />
          )}
        </main>
      </div>
    </div>
  );
}

setRoute((query.get("route") ?? "settings") as Route);
const root = createRoot(document.getElementById("root")!);
root.render(
  <StrictMode>
    <Profiler
      id="theme-shell"
      onRender={() => {
        commits += 1;
      }}
    >
      <SettingsProvider>
        <Shell />
      </SettingsProvider>
    </Profiler>
  </StrictMode>
);
