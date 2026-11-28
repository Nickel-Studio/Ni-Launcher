import { useState } from "react";
import { createRoot } from "react-dom/client";
import { SkinAuthSettings } from "../../src/components/settings/SkinAuthSettings";
import { isBuiltinSkinSite, skinSiteFavicon } from "../../src/lib/skinAuth";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";

const bridge = window as any;
const calls: any[] = [];
const clients = new Map<string, { clientId: string; clientSecret: string }>();
bridge.__feature = { calls, isBuiltinSkinSite, skinSiteFavicon };
bridge.__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: any = {}) => {
    calls.push({ command, args });
    const key = JSON.stringify([args.server, args.root, args.versionId]);
    if (command === "skin_oauth_config_get") {
      if (isBuiltinSkinSite(String(args.server))) throw new Error("Built-in credentials must not be requested");
      return {
        ...(clients.get(key) ?? { clientId: "fixture-client", clientSecret: "fixture-secret-long-value" }),
        source: "custom", redirectUri: "nickel://auth",
      };
    }
    if (command === "skin_oauth_config_set") {
      if (args.config) clients.set(key, args.config);
      else clients.delete(key);
      return;
    }
    if (command === "skin_site_describe") return {
      server: args.server, api: `${args.server}/api/yggdrasil`,
      name: args.server === "https://a.example" ? "Alpha Skin" : "Beta Skin",
      homepage: args.server, register: "",
    };
    throw new Error(`Unexpected command ${command}`);
  },
};
function Fixture() {
  const [method, setMethod] = useState<"classic" | "oauth2">("classic");
  const [server, setServer] = useState("https://a.example");
  const [scope, setScope] = useState<{ root?: string; versionId?: string }>({});
  bridge.__feature.setServer = setServer;
  bridge.__feature.setScope = setScope;
  return (
    <main style={{ padding: 30 }}>
      <div style={{ width: 420, maxWidth: "100%" }}>
        <SkinAuthSettings {...scope} server={server} method={method} onChange={(value) => setMethod(value ?? "classic")} />
      </div>
      <button onClick={() => setServer(server === "https://a.example" ? "https://b.example" : "https://a.example")}>Switch site</button>
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
