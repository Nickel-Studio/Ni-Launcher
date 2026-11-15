/** Production instance hook/navigation with synthetic, browser-local IPC only. */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { SettingsProvider } from "../../src/lib/settings";
import {
  getLaunchInstance,
  getVersionFocus,
  setLaunchInstance,
  setVersionFocus,
} from "../../src/lib/instances";
import { VersionRootNav } from "../../src/components/version/VersionRootNav";
import {
  useInstanceRegistry,
  type UseInstanceRegistry,
} from "../../src/components/version/useInstances";
import { orderInstanceRoots } from "../../src/components/version/instancesApi";
import Versions from "../../src/pages/Versions";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/pages/Versions.css";

type Screen = "nav" | "versions";
type Registry = {
  roots: string[];
  active: string | null;
  labels: Record<string, string>;
};
type Seed = {
  registry: Registry;
  gameDir: string;
  defaultDir: string;
  portable?: boolean;
  validDirs?: string[];
  screen?: Screen;
};
type Disk = {
  registry: Registry;
  game: Record<string, unknown>;
  defaultDir: string;
  portable: boolean;
  validDirs: string[];
  screen: Screen;
  pickedDir: string | null;
};
type Call = { command: string; args: Record<string, unknown> };
type MockWindow = Window & {
  __rootOrderSeed?: Seed;
  __rootOrder?: typeof controls;
  __TAURI_INTERNALS__: {
    invoke: (
      command: string,
      args?: Record<string, unknown>
    ) => Promise<unknown>;
    transformCallback: (
      callback: (payload: unknown) => void,
      once?: boolean
    ) => number;
    unregisterCallback: (id: number) => void;
    convertFileSrc: (path: string) => string;
  };
  __TAURI_EVENT_PLUGIN_INTERNALS__: {
    unregisterListener: (event: string, eventId: number) => void;
  };
};

const fixtureWindow = window as unknown as MockWindow;
const STORE_KEY = "nickel:test:instance-root-order:disk:v1";
const CALLS_KEY = "nickel:test:instance-root-order:ipc:v1";
const errors: string[] = [];
const unexpected: string[] = [];
let currentRegistry: UseInstanceRegistry | null = null;
let mountCount = 0;
let mounted = false;
let callbackId = 0;
let gameWriteGate: Promise<void> | null = null;
let releaseGameWrite: (() => void) | null = null;

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function fromSeed(seed: Seed): Disk {
  return {
    registry: clone(seed.registry),
    game: { gameDir: seed.gameDir, memoryMb: 4096, memoryMode: "auto" },
    defaultDir: seed.defaultDir,
    portable: seed.portable === true,
    validDirs: [...(seed.validDirs ?? [])],
    screen: seed.screen ?? "nav",
    pickedDir: null,
  };
}

function readDisk(): Disk {
  const raw = window.localStorage.getItem(STORE_KEY);
  if (!raw) throw new Error("Synthetic IPC seed is missing");
  return JSON.parse(raw) as Disk;
}

function writeDisk(disk: Disk): void {
  window.localStorage.setItem(STORE_KEY, JSON.stringify(disk));
}

function readCalls(): Call[] {
  return JSON.parse(window.localStorage.getItem(CALLS_KEY) ?? "[]") as Call[];
}

function record(command: string, args: Record<string, unknown>): void {
  const calls = readCalls();
  calls.push({ command, args: clone(args) });
  window.localStorage.setItem(CALLS_KEY, JSON.stringify(calls));
}

if (!window.localStorage.getItem(STORE_KEY)) {
  writeDisk(
    fromSeed(
      fixtureWindow.__rootOrderSeed ?? {
        registry: {
          roots: ["X:/nickel-order-fixture/default"],
          active: null,
          labels: {},
        },
        gameDir: "X:/nickel-order-fixture/default",
        defaultDir: "X:/nickel-order-fixture/default",
      }
    )
  );
}

// Writes are stored verbatim: the mock never implements root ordering or deduplication.
fixtureWindow.__TAURI_INTERNALS__ = {
  invoke: async (command, args = {}) => {
    record(command, args);
    const disk = readDisk();
    switch (command) {
      case "instances_get": {
        if (!disk.portable) return JSON.stringify(disk.registry);
        // This models the effective portable IPC response, not native manifest discovery.
        return JSON.stringify({
          ...disk.registry,
          roots: [
            disk.defaultDir,
            ...disk.registry.roots.filter(
              (dir) => disk.validDirs.includes(dir) && dir !== disk.defaultDir
            ),
          ],
          active: disk.defaultDir,
        });
      }
      case "instances_set": {
        if (typeof args.json !== "string")
          throw new Error("Missing registry JSON");
        disk.registry = JSON.parse(args.json) as Registry;
        writeDisk(disk);
        return null;
      }
      case "game_settings_get":
        return JSON.stringify(disk.game);
      case "game_settings_set": {
        if (typeof args.json !== "string") throw new Error("Missing game JSON");
        if (gameWriteGate) await gameWriteGate;
        const latest = readDisk();
        latest.game = JSON.parse(args.json) as Record<string, unknown>;
        writeDisk(latest);
        return null;
      }
      case "instance_default_dir":
        return disk.defaultDir;
      case "instance_ensure_default":
        if (!disk.registry.roots.includes(disk.defaultDir)) {
          disk.registry.roots.unshift(disk.defaultDir);
        }
        if (!disk.registry.active) disk.registry.active = disk.defaultDir;
        writeDisk(disk);
        return disk.defaultDir;
      case "instance_root_rename": {
        const dir = String(args.dir ?? "");
        disk.registry.labels[dir] = String(args.label ?? "");
        writeDisk(disk);
        return null;
      }
      case "instance_scan":
        return "[]";
      case "settings_get":
        return JSON.stringify({
          themeMode: "dark",
          accent: "mono",
          instanceNamingOptimize: false,
        });
      case "settings_set":
        return null;
      case "instance_suggest_label":
        return null;
      case "plugin:dialog|open": {
        const result = disk.pickedDir;
        disk.pickedDir = null;
        writeDisk(disk);
        return result;
      }
      case "plugin:dialog|message":
        return "Ok";
      case "app_tasks_snapshot":
        return [];
      case "plugin:event|listen":
        return ++callbackId;
      case "plugin:event|unlisten":
        return null;
      default:
        unexpected.push(command);
        throw new Error(`IPC outside this synthetic fixture: ${command}`);
    }
  },
  transformCallback: () => ++callbackId,
  unregisterCallback: () => {},
  convertFileSrc: (path) => path,
};
fixtureWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};

document.documentElement.dataset.theme = "dark";
document.documentElement.dataset.accent = "mono";
const appRoot = createRoot(document.getElementById("root") as HTMLElement);

async function settle(): Promise<void> {
  await new Promise<void>((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  );
}

function run(action: Promise<void>): void {
  void action.catch((error: unknown) => {
    errors.push(error instanceof Error ? error.message : String(error));
  });
}

function HookNavigation() {
  const registry = useInstanceRegistry();
  const [nextDir, setNextDir] = useState("");
  currentRegistry = registry;
  return (
    <div className="root-order-layout">
      <VersionRootNav
        roots={registry.roots}
        labels={registry.labels}
        defaultDir={registry.defaultDir}
        active={registry.active}
        busy={registry.loading || registry.busy}
        onSelect={(dir) => run(registry.activate(dir))}
        onAdd={() => run(registry.addRoot(nextDir))}
        onCreate={() => run(registry.addRoot(nextDir))}
        onImport={() => {}}
        onRemove={(dir) => run(registry.removeRoot(dir))}
        onDelete={() =>
          errors.push("Disk deletion is outside this synthetic fixture")
        }
        onRename={() => {}}
        onOpen={() => {}}
        onRefresh={() => registry.reload()}
      />
      <section className="root-order-status">
        <h2>Production hook and navigation</h2>
        <p>
          Mocked IPC; synthetic paths; no native account or registry access.
        </p>
        <label>
          Synthetic directory
          <input
            aria-label="Synthetic directory"
            value={nextDir}
            onChange={(event) => setNextDir(event.target.value)}
          />
        </label>
        <output aria-label="Hook status">
          {registry.loading ? "loading" : registry.busy ? "busy" : "ready"}
        </output>
        <pre>
          {JSON.stringify(
            {
              roots: registry.roots,
              active: registry.active,
              defaultDir: registry.defaultDir,
              error: registry.error,
            },
            null,
            2
          )}
        </pre>
      </section>
    </div>
  );
}

function mount(screen: Screen = readDisk().screen): void {
  const disk = readDisk();
  disk.screen = screen;
  writeDisk(disk);
  currentRegistry = null;
  mountCount += 1;
  mounted = true;
  appRoot.render(
    screen === "versions" ? (
      <SettingsProvider>
        <Versions />
      </SettingsProvider>
    ) : (
      <HookNavigation />
    )
  );
}

async function unmount(): Promise<void> {
  appRoot.render(null);
  await settle();
  currentRegistry = null;
  mounted = false;
}

function snapshot() {
  const rows = Array.from(
    document.querySelectorAll<HTMLButtonElement>(".vs-nav__list .vs-nav__item")
  );
  return {
    scope:
      "production hook/navigation or Versions page; mocked IPC; synthetic browser-local persistence; not native acceptance",
    mounted,
    mountCount,
    screen: readDisk().screen,
    disk: clone(readDisk()),
    references: { launch: getLaunchInstance(), focus: getVersionFocus() },
    hook: currentRegistry
      ? {
          roots: [...currentRegistry.roots],
          active: currentRegistry.active,
          defaultDir: currentRegistry.defaultDir,
          loading: currentRegistry.loading,
          busy: currentRegistry.busy,
          error: currentRegistry.error,
        }
      : null,
    ui: {
      names: rows.map(
        (row) => row.querySelector(".vs-nav__name")?.textContent ?? ""
      ),
      paths: rows.map(
        (row) => row.querySelector(".vs-nav__path")?.textContent ?? ""
      ),
      activeNames: rows
        .filter((row) => row.getAttribute("aria-current") === "true")
        .map((row) => row.querySelector(".vs-nav__name")?.textContent ?? ""),
      disabled: rows.map((row) => row.disabled),
      rows: rows.map((row) => {
        const rect = row.getBoundingClientRect();
        return {
          y: rect.y,
          width: rect.width,
          height: rect.height,
          visible:
            rect.width > 0 &&
            rect.height > 0 &&
            getComputedStyle(row).visibility !== "hidden",
        };
      }),
      loading: Boolean(document.querySelector(".vs-root--loading")),
      errors: Array.from(
        document.querySelectorAll(".vs-alert, .nw-loading--error")
      ).map((node) => node.textContent ?? ""),
    },
    calls: readCalls(),
    unexpected: [...unexpected],
    errors: [...errors],
  };
}

const controls = {
  snapshot,
  order: (roots: string[], defaultDir: string) => {
    const before = [...roots];
    return {
      roots: orderInstanceRoots(roots, defaultDir),
      inputUnchanged: JSON.stringify(before) === JSON.stringify(roots),
    };
  },
  reset: async (seed: Seed) => {
    await unmount();
    errors.length = 0;
    unexpected.length = 0;
    window.localStorage.setItem(CALLS_KEY, "[]");
    writeDisk(fromSeed(seed));
    mount(seed.screen ?? "nav");
    await settle();
  },
  mount: async (screen?: Screen) => {
    mount(screen);
    await settle();
  },
  unmount,
  reload: async () => {
    if (!currentRegistry) throw new Error("Hook navigation is not mounted");
    currentRegistry.reload();
    await settle();
  },
  activate: async (dir: string) => {
    if (!currentRegistry) throw new Error("Hook navigation is not mounted");
    await currentRegistry.activate(dir);
    await settle();
  },
  addRoot: async (dir: string) => {
    if (!currentRegistry) throw new Error("Hook navigation is not mounted");
    await currentRegistry.addRoot(dir);
    await settle();
  },
  removeRoot: async (dir: string) => {
    if (!currentRegistry) throw new Error("Hook navigation is not mounted");
    await currentRegistry.removeRoot(dir);
    await settle();
  },
  renameRoot: async (dir: string, label: string) => {
    if (!currentRegistry) throw new Error("Hook navigation is not mounted");
    await currentRegistry.renameRoot(dir, label);
    await settle();
  },
  references: (dir: string) => {
    setLaunchInstance({
      root: dir,
      versionId: "synthetic-version",
      loader: "vanilla",
      icon: "grass",
    });
    setVersionFocus({ root: dir, versionId: "synthetic-version" });
  },
  holdGameWrite: () => {
    gameWriteGate = new Promise<void>((resolve) => {
      releaseGameWrite = resolve;
    });
  },
  releaseGameWrite: () => {
    releaseGameWrite?.();
    gameWriteGate = null;
    releaseGameWrite = null;
  },
  externalAppend: (dir: string) => {
    const disk = readDisk();
    disk.registry.roots.push(dir);
    writeDisk(disk);
  },
  pick: (dir: string) => {
    const disk = readDisk();
    disk.pickedDir = dir;
    writeDisk(disk);
  },
};
fixtureWindow.__rootOrder = controls;
mount();
