import { useState } from "react";
import { createRoot } from "react-dom/client";
import { Modal, Button, LoadingCard } from "../../src/components/ui";
import { ThemeArtwork } from "../../src/components/ui/ThemeArtwork";
import { SkinAuthSettings } from "../../src/components/settings/SkinAuthSettings";
import { isBuiltinSkinSite, skinSiteFavicon } from "../../src/lib/skinAuth";
import { McVersionCombobox } from "../../src/components/community/McVersionCombobox";
import { CommunityPagination } from "../../src/components/community/CommunityPagination";
import { BackToTopBubble } from "../../src/components/download/BackToTopBubble";
import { DownloadBubble } from "../../src/components/download/DownloadBubble";
import { refreshDownloads } from "../../src/components/download/store";
import {
  CharacterAvatar,
  type CharacterAvatarProps,
} from "../../src/components/account/CharacterAvatar";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/components/settings/settings.css";
import "../../src/components/community/community.css";

const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
const avatarRequests: Array<Record<string, unknown>> = [];
const clients = new Map<string, { clientId: string; clientSecret: string }>();
const bridge = window as any;
let taskStatus = "";
let revision = 0;
bridge.__feature = {
  calls,
  avatarRequests,
  skinReply: () => null,
  clients,
  isBuiltinSkinSite,
  skinSiteFavicon,
  versionCommits: 0,
  showTask: async (status: string) => {
    taskStatus = status;
    revision++;
    await refreshDownloads();
  },
};
bridge.__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: Record<string, unknown> = {}) => {
    calls.push({ command, args });
    if (command === "skin_fetch") {
      avatarRequests.push(args);
      return bridge.__feature.skinReply(args);
    }
    const key = JSON.stringify([args.server, args.root, args.versionId]);
    if (command === "skin_oauth_config_get") {
      if (isBuiltinSkinSite(String(args.server)))
        throw new Error("Built-in credentials must not be requested");
      return {
        ...(clients.get(key) ?? {
          clientId: "fixture-client",
          clientSecret: "fixture-secret-long-value",
        }),
        source: "custom",
        redirectUri: "nickel://auth",
      };
    }
    if (command === "skin_oauth_config_set") {
      if (args.config) clients.set(key, args.config as any);
      else clients.delete(key);
      return;
    }
    if (command === "skin_site_describe")
      return {
        server: args.server,
        api: `${args.server}/api/yggdrasil`,
        name: args.server === "https://a.example" ? "Alpha Skin" : "Beta Skin",
        homepage: args.server,
        register: "",
      };
    if (command === "app_tasks_snapshot")
      return taskStatus
        ? [
            {
              taskId: "fixture-download",
              revision,
              kind: "resource",
              target: "",
              label: "Fixture task",
              status: taskStatus,
              cancelRequested: false,
              stage: "Download",
              detail: "",
              percent: 0.5,
              filesDone: 0,
              filesTotal: 1,
              transferredBytes: 128,
              completedBytes: 0,
              totalBytes: 256,
              speedBps: 128,
              remainingMs: 1000,
              startedAt: Date.now(),
              endedAt: null,
              message: "",
              files: [],
              stages: [],
            },
          ]
        : [];
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return;
    throw new Error(`Unexpected command ${command}`);
  },
  transformCallback: () => 1,
  unregisterCallback: () => {},
  convertFileSrc: (s: string) => s,
};
bridge.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
function AvatarFixture() {
  const [props, setProps] = useState<CharacterAvatarProps>({
    name: "Twin",
    source: "littleskin",
    server: "https://a.example",
    uuid: "1".repeat(32),
  });
  bridge.__feature.setAvatar = setProps;
  return (
    <div id="avatar-fixture">
      <CharacterAvatar {...props} />
    </div>
  );
}
function Fixture() {
  const [open, setOpen] = useState(false);
  const [method, setMethod] = useState<"classic" | "oauth2">("classic");
  const [server, setServer] = useState("https://a.example");
  const [scope, setScope] = useState<{ root?: string; versionId?: string }>({});
  bridge.__feature.setServer = setServer;
  bridge.__feature.setScope = setScope;
  const [version, setVersion] = useState("");
  const [page, setPage] = useState(3);
  const [target, setTarget] = useState<HTMLDivElement | null>(null);
  const [avatar, setAvatar] = useState(false);
  bridge.__feature.showAvatar = () => setAvatar(true);
  bridge.__feature.setModal = setOpen;
  return (
    <main style={{ padding: 30, height: "100vh", overflow: "auto" }}>
      <Button onClick={() => setOpen(true)}>Open modal</Button>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title="Retained modal"
        footer={<Button onClick={() => setOpen(false)}>Cancel modal</Button>}
      >
        <input aria-label="Modal input" defaultValue="Retained content" />
      </Modal>
      <div style={{ width: 420, marginTop: 20 }}>
        <SkinAuthSettings
          {...scope}
          server={server}
          method={method}
          onChange={(value) => setMethod(value ?? "classic")}
        />
      </div>
      <Button
        onClick={() =>
          setServer(
            server === "https://a.example"
              ? "https://b.example"
              : "https://a.example"
          )
        }
      >
        Switch site
      </Button>
      <div style={{ width: 220, marginTop: 20 }}>
        <McVersionCombobox
          versions={[
            "1.21.4",
            "1.21.1",
            "1.20.6",
            "1.20.1",
            ...Array.from({ length: 110 }, (_, i) => `0.${i}.0`),
            "26.2-snapshot",
          ]}
          value={version}
          onChange={(value) => {
            bridge.__feature.versionCommits++;
            setVersion(value);
          }}
          ariaLabel="MC version"
        />
        <output id="version">{version}</output>
      </div>
      <CommunityPagination
        page={page}
        hasMore={page < 4}
        onFirst={() => setPage(1)}
        onPrevious={() => setPage((p) => p - 1)}
        onNext={() => setPage((p) => p + 1)}
      />
      <div
        ref={setTarget}
        id="scroll-target"
        style={{ height: 150, overflowY: "auto", marginTop: 20 }}
      >
        <div style={{ height: 1400 }}>Scrollable content</div>
      </div>
      <BackToTopBubble target={target} />
      <DownloadBubble />
      <div
        id="theme-bar"
        className="ni-titlebar"
        style={{ height: 64, position: "relative" }}
      >
        <ThemeArtwork accent="copper" placement="titlebar" />
      </div>
      <LoadingCard size="inline" label="Mining" />
      <LoadingCard size="inline" state="error" label="Mining failed" />
      {avatar && <AvatarFixture />}
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
