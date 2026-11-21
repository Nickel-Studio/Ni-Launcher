import { useCallback, useState } from "react";
import { SettingsProvider } from "./lib/settings";
import { useAppNav } from "./lib/nav";
import { TitleBar } from "./components/TitleBar";
import { SplashScreen } from "./components/SplashScreen";
import { AnimatedSwap, BackgroundLayer, ToastHost } from "./components/ui";
import { DownloadOverlay } from "./components/download";
import { PortableBootstrap } from "./components/PortableBootstrap";
import { StartupGate } from "./components/startup/StartupGate";
import { LauncherProvider } from "./components/launch/useLauncher";
import Home from "./pages/Home";
import Resources from "./pages/Resources";
import Settings from "./pages/Settings";
import More from "./pages/More";
import DownloadDetail from "./pages/DownloadDetail";
import Versions from "./pages/Versions";
import VersionSettings from "./pages/VersionSettings";

/** 应用外壳；下载浮窗不随路由卸载，全局只挂载一次。 */
function Shell({ blocked }: { blocked: boolean }) {
  const { route } = useAppNav();

  return (
    <div className="ni-shell" inert={blocked}>
      <BackgroundLayer />
      <TitleBar />
      <div className="ni-body">
        <main className="ni-content">
          <AnimatedSwap swapKey={route}>
            {route === "home" ? (
              <Home />
            ) : route === "settings" ? (
              <Settings />
            ) : route === "more" ? (
              <More />
            ) : route === "instances" ? (
              <Versions />
            ) : route === "versionSettings" ? (
              <VersionSettings />
            ) : route === "downloads" ? (
              <DownloadDetail />
            ) : (
              <Resources />
            )}
          </AnimatedSwap>
        </main>
      </div>
      <DownloadOverlay />
      <ToastHost />
    </div>
  );
}

export default function App() {
  const [ready, setReady] = useState(false);
  const finishStartup = useCallback(() => setReady(true), []);
  return (
    <SettingsProvider>
      <SplashScreen />
      <LauncherProvider>
        <Shell blocked={!ready} />
      </LauncherProvider>
      <StartupGate onReady={finishStartup} />
      {ready ? <PortableBootstrap /> : null}
    </SettingsProvider>
  );
}
