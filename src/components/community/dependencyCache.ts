import type {
  DependencyLookup,
  ModrinthHit,
  ModrinthVersion,
  ResolvedDependency,
} from "../modrinth/types";
import {
  deduplicateResolvedDependencies,
  dependencyKey,
  requiredDependencies,
  type DependencyEntry,
} from "./dependencies";

/** Owned by one retained detail page. UI disclosure never invalidates this cache. */
export function createDependencyCache(
  project: ModrinthHit,
  resolve?: (lookup: DependencyLookup) => Promise<ResolvedDependency>
) {
  const requests = new Map<string, Promise<ResolvedDependency>>();
  const failures = new Set<string>();
  const versions = new Map<string, Promise<DependencyEntry[]>>();
  const failedVersions = new Set<string>();
  const waiting: Array<() => void> = [];
  let active = 0;
  async function lookup(
    dependency: DependencyLookup
  ): Promise<ResolvedDependency> {
    const key = JSON.stringify([
      dependency.projectId || "",
      dependency.versionId || "",
    ]);
    let pending = requests.get(key);
    if (!pending) {
      pending = (async () => {
        if (active >= 4)
          await new Promise<void>((release) => waiting.push(release));
        else active++;
        try {
          if (!resolve) throw new Error("前置元数据暂不可用");
          const result = await resolve(dependency);
          if (
            dependency.versionId &&
            result.version?.id !== dependency.versionId
          )
            throw new Error("返回的前置版本与声明不一致，请重试。");
          if (result.project.source !== project.source)
            throw new Error("返回的前置来源与声明不一致，请重试。");
          return result;
        } catch (error) {
          failures.add(key);
          throw error;
        } finally {
          const next = waiting.shift();
          if (next) next();
          else active--;
        }
      })();
      requests.set(key, pending);
    }
    return pending;
  }
  function loadVersion(version: ModrinthVersion): Promise<DependencyEntry[]> {
    let pending = versions.get(version.id);
    if (!pending) {
      pending = (async () => {
        let declared = version.dependencies;
        if (declared === undefined) {
          const request = {
            projectId: project.projectId || project.slug,
            versionId: version.id,
          };
          try {
            const metadata = await lookup(request);
            if (metadata.version?.dependencies === undefined)
              throw new Error("前置元数据暂不可用");
            declared = metadata.version.dependencies;
          } catch (error) {
            failures.add(
              JSON.stringify([request.projectId || "", request.versionId])
            );
            failedVersions.add(version.id);
            return [
              {
                dependency: {
                  dependencyType: "required" as const,
                  projectId: "",
                  versionId: version.id,
                  fileName: version.name,
                },
                error: String(error instanceof Error ? error.message : error),
              },
            ];
          }
        }
        const entries = await Promise.all(
          requiredDependencies(declared, project, version).map(
            async (dependency): Promise<DependencyEntry> => {
              try {
                if (!dependency.projectId && !dependency.versionId)
                  return { dependency, error: "仅提供文件名，无法定位项目" };
                return { dependency, resolved: await lookup(dependency) };
              } catch (error) {
                failedVersions.add(version.id);
                return {
                  dependency,
                  error: String(error instanceof Error ? error.message : error),
                };
              }
            }
          )
        );
        return deduplicateResolvedDependencies(entries, project, version);
      })();
      versions.set(version.id, pending);
    }
    return pending;
  }
  return {
    async load(items: readonly ModrinthVersion[], retry = false) {
      if (retry) {
        for (const key of failures) requests.delete(key);
        for (const key of failedVersions) versions.delete(key);
        failures.clear();
        failedVersions.clear();
      }
      return new Map(
        await Promise.all(
          items.map(
            async (version) => [version.id, await loadVersion(version)] as const
          )
        )
      );
    },
  };
}

export function groupDependencies(
  items: readonly ModrinthVersion[],
  entries: ReadonlyMap<string, DependencyEntry[]>
) {
  const grouped = new Map<
    string,
    { entry: DependencyEntry; parent: ModrinthVersion; pinned: Set<string> }
  >();
  for (const parent of items) {
    for (const entry of entries.get(parent.id) ?? []) {
      const resolved = entry.resolved;
      const key = resolved
        ? `${resolved.project.source}:${
            resolved.project.projectId || resolved.project.slug
          }`
        : dependencyKey(entry.dependency);
      const existing = grouped.get(key);
      if (existing) existing.pinned.add(resolved?.version?.id ?? "");
      else
        grouped.set(key, {
          entry,
          parent,
          pinned: new Set([resolved?.version?.id ?? ""]),
        });
    }
  }
  return [...grouped.values()].map(({ entry, parent, pinned }) => ({
    entry:
      entry.resolved && pinned.size > 1
        ? { ...entry, resolved: { ...entry.resolved, version: null } }
        : entry,
    parent,
  }));
}
