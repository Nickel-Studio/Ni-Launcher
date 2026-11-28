import { Button, LoadingCard } from "../ui";
import { CommunityKindIcon } from "./CommunityKindIcon";
import { dependencyKey, dependencyKind, type DependencyEntry } from "./dependencies";
import { groupDependencies } from "./dependencyCache";
import { formatDownloads, formatIsoDate } from "../modrinth/parse";
import type { ModrinthVersion, ResolvedDependency } from "../modrinth/types";

interface Props {
  versions: readonly ModrinthVersion[];
  entries: ReadonlyMap<string, DependencyEntry[]>;
  loading: boolean;
  onRetry: () => void;
  onOpen?: (dependency: ResolvedDependency, version: ModrinthVersion) => void;
}

export function RequiredDependencies({ versions, entries, loading, onRetry, onOpen }: Props) {
  const grouped = groupDependencies(versions, entries);
  if (grouped.length === 0) return loading ? <LoadingCard size="inline" label="正在加载" /> : null;
  return (
    <div className="pd-deps" aria-busy={loading}>
      <h3 className="pd-section-title">前置资源</h3>
      <ul className="pd-deps__list">
        {grouped.map(({ entry, parent }) => {
          const resolved = entry.resolved;
          const title = resolved?.project.title || entry.dependency.fileName || entry.dependency.projectId || entry.dependency.versionId;
          return (
            <li key={resolved ? `${resolved.project.source}:${resolved.project.projectId || resolved.project.slug}` : dependencyKey(entry.dependency)} className="pd-deps__item">
              {resolved ? (
                <button type="button" className="pd-deps__project" onClick={() => onOpen?.(resolved, parent)} aria-label={`打开前置 ${title} 的详情`}>
                  <span className="pd-deps__icon" aria-hidden="true">
                    {resolved.project.iconUrl ? <img src={resolved.project.iconUrl} alt="" /> : <CommunityKindIcon kind={dependencyKind(resolved.project)} />}
                  </span>
                  <span className="pd-deps__text">
                    <span className="pd-deps__title">{title}</span>
                    {resolved.project.description ? <span className="pd-deps__meta">{resolved.project.description}</span> : null}
                    <span className="pd-deps__meta">
                      {formatDownloads(resolved.project.downloads)} 次下载 · {formatIsoDate(resolved.project.dateModified)} · {resolved.project.source === "modrinth" ? "Modrinth" : "CurseForge"}
                    </span>
                  </span>
                </button>
              ) : (
                <div className="pd-deps__unresolved">
                  <span className="pd-deps__text"><span className="pd-deps__title">{title}</span><span className="pd-deps__error" role="alert">{entry.error}</span></span>
                  {entry.error && (entry.dependency.projectId || entry.dependency.versionId) ? <Button size="sm" variant="secondary" disabled={loading} onClick={onRetry}>重试</Button> : null}
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
