import { StrictMode, useLayoutEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { SettingsProvider } from "../../../src/lib/settings";
import { setLaunchInstance, getLaunchInstance, type LaunchInstance } from "../../../src/lib/instances";
import { useLauncher, type UseLauncher, type LaunchArgs } from "../../../src/components/launch/useLauncher";
import { LaunchErrorDialog, launchErrorInfo } from "../../../src/components/launch/LaunchPanel";
import { installBackend, type Setup } from "./ipc";
import "../../../src/styles/tokens.css";
import "../../../src/styles/global.css";
import "../../../src/styles/ui.css";
import "../../../src/styles/shell.css";

declare global {
  interface Window {
    __LAUNCH_SETUP__?: Setup;
    __launchFixture: {
      snapshot: () => Record<string, unknown>;
      launch: (args?: Partial<LaunchArgs>) => void;
      cancel: () => void;
      event: (payload: Record<string, unknown>) => Promise<void>;
      release: (id: string) => void;
      select: (instance: LaunchInstance) => void;
      unmount: () => void;
      errorInfo: typeof launchErrorInfo;
    };
  }
}

const setup = window.__LAUNCH_SETUP__ || {};
const backend = installBackend(setup);
const initialInstance: LaunchInstance = {
  root: "C:/launch-acceptance/early-instance",
  versionId: "1.21.1",
  displayName: setup.displayName || "Emerald Workshop",
  loader: "fabric",
  icon: "grass",
};
setLaunchInstance(initialInstance);
document.documentElement.classList.add("nw-app-ready");
const root = createRoot(document.getElementById("root")!);
let latest: UseLauncher | null = null;
let closeCount = 0;
let logCount = 0;
const args: LaunchArgs = {
  versionId: initialInstance.versionId,
  gameDir: initialInstance.root,
  username: "AcceptancePlayer",
  memoryMb: 4096,
  source: "official_auto",
};

function HookHarness() {
  const launcher = useLauncher();
  useLayoutEffect(() => { latest = launcher; });
  return <output data-testid="hook-state">{JSON.stringify(launcher)}</output>;
}

function DialogHarness() {
  const [open, setOpen] = useState(true);
  const info = launchErrorInfo(setup.error || "OutOfMemoryError\ncomplete stack stays in report", setup.diagnostic || null)!;
  return <LaunchErrorDialog
    open={open}
    title={info.title}
    reason={info.reason}
    message={info.message}
    onClose={() => { closeCount += 1; setOpen(false); }}
    onOpenLogs={() => { logCount += 1; void invoke("launch_log_open", { dir: args.gameDir, versionId: args.versionId }); }}
    onExportReport={() => invoke<string | null>("launch_error_report_export", { dir: args.gameDir, versionId: args.versionId, failure: setup.error || info.reason })}
  />;
}

window.__launchFixture = {
  snapshot: () => ({
    ...backend.snapshot(),
    state: latest ? JSON.parse(JSON.stringify(latest)) : null,
    selectedInstance: getLaunchInstance(),
    closeCount,
    logCount,
  }),
  launch: (patch = {}) => { if (latest) void latest.launch({ ...args, ...patch }); },
  cancel: () => latest?.cancel(),
  event: backend.emit,
  release: backend.release,
  select: setLaunchInstance,
  unmount: () => root.unmount(),
  errorInfo: launchErrorInfo,
};

if (setup.mode === "hook") {
  root.render(<StrictMode><HookHarness /></StrictMode>);
} else if (setup.mode === "dialog") {
  root.render(<StrictMode><DialogHarness /></StrictMode>);
} else {
  const { Home } = await import("../../../src/pages/Home");
  root.render(
    <StrictMode>
      <SettingsProvider>
        <div className="nw-shell">
          <header className="nw-titlebar">Nickel</header>
          <div className="nw-body"><main className="nw-content"><Home /></main></div>
        </div>
      </SettingsProvider>
    </StrictMode>,
  );
}
