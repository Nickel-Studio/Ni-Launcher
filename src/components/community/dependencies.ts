import type {
  ModrinthHit,
  ModrinthKind,
  ModrinthVersion,
  ProjectDependency,
  ResolvedDependency,
} from "../modrinth/types";

export function dependencyKind(project: ModrinthHit): ModrinthKind {
  const kinds: Record<string, ModrinthKind> = {
    mod: "mod",
    modpack: "modpack",
    resourcepack: "resourcepack",
    shader: "shader",
    datapack: "datapack",
    "cf-class-6": "mod",
    "cf-class-4471": "modpack",
    "cf-class-12": "resourcepack",
    "cf-class-6552": "shader",
    "cf-class-6945": "datapack",
  };
  return kinds[project.projectType] ?? "mod";
}

export function projectIdentity(project: ModrinthHit): string {
  return `${project.source}:${project.projectId || project.slug}`;
}

export function dependencyKey(dependency: ProjectDependency): string {
  return JSON.stringify([
    dependency.projectId,
    dependency.versionId,
    dependency.fileName,
  ]);
}

// Loader APIs are not ordinary prerequisites.
export function isLoaderApi(
  source: ModrinthHit["source"],
  id: string
): boolean {
  return source === "curseforge"
    ? ["306612", "634179"].includes(id)
    : [
        "P7dR8mSH",
        "qvIfYCYJ",
        "fabric-api",
        "qsl",
        "quilted-fabric-api",
      ].includes(id);
}

// Keep different pinned versions; collapse repeated and unpinned declarations.
export function requiredDependencies(
  dependencies: readonly ProjectDependency[],
  project: ModrinthHit,
  version: ModrinthVersion
): ProjectDependency[] {
  const ownIds = new Set(
    [project.projectId, project.slug, version.projectId].filter(Boolean)
  );
  const required = dependencies.filter(
    (dependency) =>
      dependency.dependencyType === "required" &&
      !isLoaderApi(project.source, dependency.projectId) &&
      !(dependency.projectId && ownIds.has(dependency.projectId)) &&
      dependency.versionId !== version.id
  );
  const pinned = new Set(
    required
      .filter((dependency) => dependency.projectId && dependency.versionId)
      .map((dependency) => dependency.projectId)
  );
  const unique = new Map<string, ProjectDependency>();
  for (const dependency of required) {
    if (!dependency.versionId && pinned.has(dependency.projectId)) continue;
    const key =
      dependency.projectId || dependency.versionId
        ? JSON.stringify([dependency.projectId, dependency.versionId])
        : JSON.stringify(["file", dependency.fileName]);
    if (!unique.has(key)) unique.set(key, dependency);
  }
  return [...unique.values()];
}

export interface DependencyEntry {
  dependency: ProjectDependency;
  resolved?: ResolvedDependency;
  error?: string;
}

// Version-only records can reveal aliases or self references only after lookup.
export function deduplicateResolvedDependencies(
  entries: readonly DependencyEntry[],
  project: ModrinthHit,
  version?: ModrinthVersion
): DependencyEntry[] {
  const ownIds = new Set(
    [project.projectId, project.slug, version?.projectId].filter(Boolean)
  );
  const visible = entries.filter((entry) => {
    const resolved = entry.resolved;
    if (
      resolved &&
      (isLoaderApi(resolved.project.source, resolved.project.projectId || "") ||
        isLoaderApi(resolved.project.source, resolved.project.slug))
    )
      return false;
    return (
      !resolved ||
      resolved.project.source !== project.source ||
      ![
        resolved.project.projectId,
        resolved.project.slug,
        resolved.version?.projectId,
      ].some((id) => id && ownIds.has(id))
    );
  });
  const pinned = new Set(
    visible
      .filter((entry) => entry.resolved?.version)
      .map((entry) => projectIdentity(entry.resolved!.project))
  );
  const unique = new Map<string, DependencyEntry>();
  for (const entry of visible) {
    if (entry.resolved) {
      const identity = projectIdentity(entry.resolved.project);
      if (!entry.resolved.version && pinned.has(identity)) continue;
      const key = JSON.stringify([identity, entry.resolved.version?.id ?? ""]);
      if (!unique.has(key)) unique.set(key, entry);
    } else {
      unique.set(dependencyKey(entry.dependency), entry);
    }
  }
  return [...unique.values()];
}

// Preserve existing version groups while making an exact requirement easy to find.
export function prioritizeDependencyVersion<
  T extends { items: ModrinthVersion[] }
>(groups: readonly T[], versionId?: string): T[] {
  if (!versionId) return [...groups];
  return groups
    .map((group) => ({
      ...group,
      items: [...group.items].sort(
        (a, b) => Number(b.id === versionId) - Number(a.id === versionId)
      ),
    }))
    .sort(
      (a, b) =>
        Number(b.items.some((item) => item.id === versionId)) -
        Number(a.items.some((item) => item.id === versionId))
    );
}

export interface ResourceScrollSnapshot {
  positions: Array<{ node: HTMLElement; top: number; left: number }>;
  trigger: HTMLElement | null;
}

export function captureResourceScroll(
  host: HTMLElement | null
): ResourceScrollSnapshot {
  const nodes = host
    ? Array.from(host.querySelectorAll<HTMLElement>("*")).filter((node) => {
        if (node.scrollTop > 0 || node.scrollLeft > 0) return true;
        if (
          node.scrollHeight <= node.clientHeight &&
          node.scrollWidth <= node.clientWidth
        )
          return false;
        const style = getComputedStyle(node);
        return (
          /auto|scroll/.test(style.overflowY) ||
          /auto|scroll/.test(style.overflowX)
        );
      })
    : [];
  let parent = host;
  // Include zero positions: a nested detail may scroll an initially unscrolled ancestor.
  while (parent) {
    nodes.push(parent);
    parent = parent.parentElement;
  }
  return {
    positions: nodes.map((node) => ({
      node,
      top: node.scrollTop,
      left: node.scrollLeft,
    })),
    trigger:
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null,
  };
}

// A retained parent keeps its DOM, filters and disclosure state intact.
export function restoreResourceScroll(snapshot: ResourceScrollSnapshot): void {
  for (const { node, top, left } of snapshot.positions) {
    if (!node.isConnected) continue;
    node.scrollTop = top;
    node.scrollLeft = left;
  }
  if (snapshot.trigger?.isConnected)
    snapshot.trigger.focus({ preventScroll: true });
}
