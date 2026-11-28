import { createRoot } from "react-dom/client";
import { SettingsProvider } from "../../src/lib/settings";
import { setLaunchInstance } from "../../src/lib/instances";
import Resources from "../../src/pages/Resources";
import type {
  HitSource,
  ModrinthHit,
  ModrinthVersion,
} from "../../src/components/modrinth/types";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";
import "../../src/styles/ui.css";
import "../../src/styles/shell.css";

interface Call {
  command: string;
  args: Record<string, unknown>;
}
interface Save {
  command: string;
  args: Record<string, unknown>;
  taskId: string | null;
}
const calls: Call[] = [];
const saves: Save[] = [];
const unexpected: string[] = [];
const attempts: Record<string, number> = {};
const failures = new Map<string, number>([
  ["modrinth:UnstableMR", 1],
  ["curseforge:10103", 1],
]);
const historyFailures = new Map<string, number>([["PinnedMR", 1]]);
const metadataFailures = new Map<string, number>();
const scenario =
  new URLSearchParams(location.search).get("scenario") ?? "normal";
if (scenario === "metadata") {
  metadataFailures.set("modrinth:ParentMR", 1);
  metadataFailures.set("curseforge:10000", 1);
}
const rootDir = "C:/NickelDependencyFixture";
const target = {
  root: rootDir,
  versionId: "fixture-instance",
  loader: "fabric",
  icon: "fabric",
};
let saveMode: "cancel" | "save" = "cancel";
let taskSequence = 0;
let callbackSequence = 0;
const callbacks = new Map<number, (payload: unknown) => void>();

function hit(
  projectId: string,
  slug: string,
  title: string,
  source: HitSource
): ModrinthHit {
  return {
    projectId,
    slug,
    title,
    source,
    description: "Nickel community dependency fixture",
    projectType: source === "curseforge" ? "cf-class-6" : "mod",
    iconUrl: "",
    downloads: 1000,
    author: "Nickel Test Fixture",
    dateModified: "2026-10-02T00:00:00Z",
    versions: ["1.21.1", "1.20.1"],
    loaders: ["fabric", "quilt"],
  };
}
const projects = new Map<string, ModrinthHit>([
  [
    "ParentMR",
    hit(
      "ParentMR",
      "shared-community-mod",
      "Modrinth dependency regression",
      "modrinth"
    ),
  ],
  ["FabricAPI", hit("FabricAPI", "fabric-api", "Fabric API", "modrinth")],
  ["QuiltAPI", hit("QuiltAPI", "quilted-fabric-api", "Quilt API", "modrinth")],
  ["PinnedMR", hit("PinnedMR", "pinned-mr", "Pinned dependency", "modrinth")],
  [
    "SameNameA",
    hit("SameNameA", "same-name-a", "Same named required project", "modrinth"),
  ],
  [
    "SameNameB",
    hit("SameNameB", "same-name-b", "Same named required project", "modrinth"),
  ],
  [
    "UnstableMR",
    hit("UnstableMR", "unstable-mr", "Retry dependency", "modrinth"),
  ],
  [
    "WrongPinnedMR",
    hit(
      "WrongPinnedMR",
      "wrong-pinned-mr",
      "Strict pinned dependency",
      "modrinth"
    ),
  ],
  [
    "IdentityMR",
    hit("IdentityMR", "identity-mr", "Identity checked dependency", "modrinth"),
  ],
  [
    "OldOnlyMR",
    hit("OldOnlyMR", "old-only-mr", "Historical dependency only", "modrinth"),
  ],
  ["SubMR", hit("SubMR", "nested-mr", "Nested dependency", "modrinth")],
  [
    "10000",
    hit(
      "10000",
      "shared-community-mod",
      "CurseForge dependency regression",
      "curseforge"
    ),
  ],
  ["306612", hit("306612", "fabric-api", "Fabric API", "curseforge")],
  ["634179", hit("634179", "quilted-fabric-api", "Quilt API", "curseforge")],
  [
    "10101",
    hit("10101", "same-name-a", "Same named required project", "curseforge"),
  ],
  [
    "10102",
    hit("10102", "same-name-b", "Same named required project", "curseforge"),
  ],
  ["10103", hit("10103", "unstable-cf", "Retry dependency", "curseforge")],
  [
    "10104",
    hit("10104", "old-only-cf", "Historical dependency only", "curseforge"),
  ],
]);
const aliases = new Map<string, string>([
  ["fabric-api", "FabricAPI"],
  ["quilted-fabric-api", "QuiltAPI"],
]);

function version(
  id: string,
  projectId: string,
  number: string,
  gameVersion: string,
  loader = "fabric",
  dependencies: unknown = [],
  datePublished = "2026-10-02T00:00:00Z"
): ModrinthVersion {
  return {
    id,
    projectId,
    name: `${projectId} ${number}`,
    versionNumber: number,
    gameVersions: [gameVersion],
    loaders: [loader],
    datePublished,
    releaseType: "release",
    files: [
      {
        filename: `${projectId}-${number}.jar`,
        url: `https://example.invalid/${id}.jar`,
        sizeBytes: 32768,
        primary: true,
        hashSha1: "",
      },
    ],
    dependencies: dependencies as ModrinthVersion["dependencies"],
  };
}
const required = (projectId = "", versionId = "", fileName = "") => ({
  dependency_type: "required",
  project_id: projectId,
  version_id: versionId,
  file_name: fileName,
});
const mrDependencies = [
  required("fabric-api"),
  required("fabric-api"),
  required("quilted-fabric-api"),
  required("PinnedMR", "ExactMR1"),
  required("PinnedMR"),
  required("", "ExactMR1"),
  required("PinnedMR", "ExactMR2"),
  required("SameNameA"),
  required("SameNameB"),
  required("UnstableMR"),
  required("WrongPinnedMR", "WrongExact1"),
  required("IdentityMR", "IdentityExact1"),
  required("", "", "manual-required.jar"),
  required("", "", "manual-required.jar"),
  required("", "", "manual-quilt.jar"),
  required("ParentMR"),
  required("", "MainMRCurrent"),
  required("", "SelfOtherMR"),
  { dependency_type: "optional", project_id: "OptionalMR" },
  { dependency_type: "incompatible", project_id: "IncompatibleMR" },
  { dependency_type: "embedded", project_id: "EmbeddedMR" },
];
const cfDependencies = [
  { relationType: 3, modId: 306612 },
  { relationType: 3, modId: 306612 },
  { relationType: 3, modId: 634179 },
  { relationType: 3, modId: 10101 },
  { relationType: 3, modId: 10102 },
  { relationType: 3, modId: 10103 },
  { relationType: 3, modId: 10000 },
  { relationType: 2, modId: 99002 },
  { relationType: 4, modId: 99004 },
  { relationType: 5, modId: 99005 },
  { relationType: "3", modId: 99003 },
  { relationType: 2, dependencyType: "required", modId: 99006 },
];
const exactVersions = new Map<string, ModrinthVersion>([
  [
    "ExactMR1",
    version(
      "ExactMR1",
      "PinnedMR",
      "2.0.0",
      "1.20.1",
      "fabric",
      [required("SubMR")],
      "2024-01-02T00:00:00Z"
    ),
  ],
  [
    "ExactMR2",
    version(
      "ExactMR2",
      "PinnedMR",
      "1.0.0",
      "1.19.4",
      "fabric",
      [],
      "2023-01-02T00:00:00Z"
    ),
  ],
  ["WrongExact1", version("WrongExact1", "WrongPinnedMR", "1.0.0", "1.20.1")],
  [
    "IdentityExact1",
    version("IdentityExact1", "IdentityMR", "1.0.0", "1.20.1"),
  ],
  ["SelfOtherMR", version("SelfOtherMR", "ParentMR", "0.9.0", "1.19.4")],
]);
const mainMr = version(
  "MainMRCurrent",
  "ParentMR",
  "5.0.0",
  "1.21.1",
  "fabric",
  mrDependencies
);
const mainCf = version(
  "50000",
  "10000",
  "5.0.0",
  "1.21.1",
  "fabric",
  cfDependencies
);
exactVersions.set(mainMr.id, mainMr);
exactVersions.set(mainCf.id, mainCf);
const histories = new Map<string, ModrinthVersion[]>([
  [
    "ParentMR",
    [
      {
        ...mainMr,
        dependencies: scenario === "metadata" ? undefined : mainMr.dependencies,
      },
      version(
        "MainMRHistory",
        "ParentMR",
        "4.0.0",
        "1.20.1",
        "quilt",
        [required("OldOnlyMR")],
        "2025-01-02T00:00:00Z"
      ),
    ],
  ],
  ["PinnedMR", [version("LatestMR", "PinnedMR", "9.0.0", "1.21.1")]],
  [
    "FabricAPI",
    [
      version("FabricNew", "FabricAPI", "0.110.0", "1.21.1"),
      version(
        "FabricOld",
        "FabricAPI",
        "0.85.0",
        "1.20.1",
        "fabric",
        [],
        "2024-01-02T00:00:00Z"
      ),
    ],
  ],
  ["QuiltAPI", [version("QuiltNew", "QuiltAPI", "7.0.0", "1.21.1", "quilt")]],
  [
    "10000",
    [
      {
        ...mainCf,
        dependencies: scenario === "metadata" ? undefined : mainCf.dependencies,
      },
      version(
        "50001",
        "10000",
        "4.0.0",
        "1.20.1",
        "quilt",
        [{ relationType: 3, modId: 10104 }],
        "2025-01-02T00:00:00Z"
      ),
    ],
  ],
  [
    "306612",
    [
      version("60002", "306612", "0.110.0", "1.21.1"),
      version(
        "60001",
        "306612",
        "0.85.0",
        "1.20.1",
        "fabric",
        [],
        "2024-01-02T00:00:00Z"
      ),
    ],
  ],
  ["634179", [version("70001", "634179", "7.0.0", "1.21.1", "quilt")]],
]);

function failOnce(
  store: Map<string, number>,
  key: string,
  message: string
): void {
  const remaining = store.get(key) ?? 0;
  if (remaining <= 0) return;
  store.set(key, remaining - 1);
  throw new Error(message);
}
function searchHits(source: HitSource, args: Record<string, unknown>) {
  const parent = projects.get(source === "modrinth" ? "ParentMR" : "10000")!;
  const all = Array.from({ length: 40 }, (_, index) =>
    index === 2 || index === 24
      ? parent
      : {
          ...hit(
            `${source}-filler-${index}`,
            `${source}-filler-${index}`,
            `Nickel Mod result ${index}`,
            source
          ),
          downloads: 900 - index,
        }
  );
  const offset = Number(args.offset ?? 0);
  const limit = Number(args.limit ?? 10);
  return JSON.stringify({
    total: all.length,
    hits: all.slice(offset, offset + limit),
  });
}
async function invoke(
  command: string,
  args: Record<string, unknown> = {}
): Promise<unknown> {
  calls.push({ command, args });
  if (command === "settings_get") return "{}";
  if (command === "settings_set" || command === "plugin:event|unlisten")
    return null;
  if (command === "plugin:event|listen") return ++callbackSequence;
  if (command === "app_tasks_snapshot") return [];
  if (command === "list_loader_provider")
    return JSON.stringify({
      loader: args.loader,
      builds: [],
      cached: true,
      elapsedMs: 0,
    });
  if (command === "preload_loader_catalog") return null;
  if (command === "instances_get")
    return JSON.stringify({ roots: [rootDir], active: rootDir });
  if (command === "resource_instance_context")
    return {
      resourceDir: rootDir,
      displayName: "Nickel fixture instance",
      minecraftVersion: "1.21.1",
      loader: "fabric",
    };
  if (command === "list_remote_versions")
    return JSON.stringify({
      latest: {},
      versions: ["1.21.1", "1.20.1", "1.19.4"].map((id) => ({
        id,
        type: "release",
        url: "",
        releaseTime: "2026-01-01T00:00:00Z",
      })),
    });
  if (command === "modrinth_search") return searchHits("modrinth", args);
  if (command === "curseforge_search") return searchHits("curseforge", args);
  if (
    command === "modrinth_project_versions" ||
    command === "curseforge_project_files"
  ) {
    const projectId = String(args.slugOrId ?? args.projectId);
    failOnce(historyFailures, projectId, "History metadata network failure");
    const values = histories.get(projectId) ?? [
      version(`${projectId}Current`, projectId, "1.0.0", "1.21.1"),
    ];
    if (command === "modrinth_project_versions") return JSON.stringify(values);
    const offset = Number(args.offset ?? 0);
    const limit = Number(args.limit ?? 50);
    return JSON.stringify({
      versions: values.slice(offset, offset + limit),
      offset,
      nextOffset: Math.min(values.length, offset + limit),
      hasMore: false,
      total: values.length,
    });
  }
  if (command === "community_dependency_resolve") {
    const source = String(args.platform) as HitSource;
    const requestedProject = String(args.projectId ?? "");
    const requestedVersion = String(args.versionId ?? "");
    const attemptKey = `${source}:${requestedProject}:${requestedVersion}`;
    attempts[attemptKey] = (attempts[attemptKey] ?? 0) + 1;
    const exact = requestedVersion
      ? exactVersions.get(requestedVersion)
      : undefined;
    const id =
      exact?.projectId ??
      (source === "modrinth"
        ? aliases.get(requestedProject) ?? requestedProject
        : requestedProject);
    if (!id) throw new Error("Filename-only dependencies cannot be resolved");
    failOnce(
      metadataFailures,
      `${source}:${id}`,
      "Required metadata network failure"
    );
    failOnce(
      failures,
      `${source}:${id}`,
      "Dependency metadata network failure"
    );
    const project = projects.get(id);
    if (!project) throw new Error(`Unknown fixture project ${id}`);
    let resolvedVersion = exact ?? null;
    if (id === "WrongPinnedMR" && attempts[attemptKey] === 1) {
      resolvedVersion = version("LatestWrong", id, "99.0.0", "1.21.1");
    }
    if (id === "IdentityMR" && attempts[attemptKey] === 1) {
      resolvedVersion = version(
        "IdentityExact1",
        "UnrelatedProject",
        "1.0.0",
        "1.20.1"
      );
    }
    return JSON.stringify({ project, version: resolvedVersion });
  }
  if (command === "modrinth_save_as" || command === "curseforge_save_as") {
    const taskId =
      saveMode === "save" ? `community-fixture-task-${++taskSequence}` : null;
    saves.push({ command, args, taskId });
    return taskId;
  }
  if (
    command === "modrinth_open_project" ||
    command === "curseforge_open_project"
  )
    return null;
  unexpected.push(command);
  throw new Error(`Unexpected fixture command: ${command}`);
}
const bridge = window as unknown as {
  __TAURI_INTERNALS__: {
    invoke: typeof invoke;
    transformCallback: (callback: (payload: unknown) => void) => number;
    unregisterCallback: (id: number) => void;
    convertFileSrc: (path: string) => string;
  };
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void };
  __communityDependencies: {
    calls: Call[];
    saves: Save[];
    unexpected: string[];
    attempts: Record<string, number>;
    scenario: string;
    setSaveMode: (mode: "cancel" | "save") => void;
    failHistory: (projectId: string, count: number) => void;
    taskCount: () => number;
    setInstance: typeof setLaunchInstance;
  };
};
bridge.__TAURI_INTERNALS__ = {
  invoke,
  transformCallback: (callback) => {
    const id = ++callbackSequence;
    callbacks.set(id, callback);
    return id;
  },
  unregisterCallback: (id) => {
    callbacks.delete(id);
  },
  convertFileSrc: (path) => path,
};
bridge.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
bridge.__communityDependencies = {
  calls,
  saves,
  unexpected,
  attempts,
  scenario,
  setSaveMode: (mode) => {
    saveMode = mode;
  },
  failHistory: (projectId, count) => {
    historyFailures.set(projectId, count);
  },
  taskCount: () => taskSequence,
  setInstance: setLaunchInstance,
};
const host = document.getElementById("root");
if (!host) throw new Error("Missing community regression root");
setLaunchInstance(target);
createRoot(host).render(
  <SettingsProvider>
    <div className="ni-shell">
      <header
        style={{
          height: "var(--titlebar-h)",
          minHeight: "var(--titlebar-h)",
          display: "flex",
          alignItems: "center",
          paddingInline: "var(--gutter)",
          background: "var(--bg)",
        }}
      >
        Nickel / 资源
      </header>
      <div className="ni-body">
        <main className="ni-content">
          <Resources />
        </main>
      </div>
    </div>
  </SettingsProvider>
);
