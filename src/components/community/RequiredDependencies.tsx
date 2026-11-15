import { useEffect, useId, useMemo, useState } from "react";
import { Button, Collapse } from "../ui";
import { IconChevronDown } from "../ui/icons";
import { CommunityKindIcon } from "./CommunityKindIcon";
import {
  deduplicateResolvedDependencies,
  dependencyKey,
  dependencyKind,
  requiredDependencies,
  type DependencyEntry,
} from "./dependencies";
import type {
  DependencyLookup,
  ModrinthHit,
  ModrinthVersion,
  ResolvedDependency,
} from "../modrinth/types";

interface Props {
  project: ModrinthHit;
  version: ModrinthVersion;
  resolve?: (dependency: DependencyLookup) => Promise<ResolvedDependency>;
  onOpen?: (dependency: ResolvedDependency, version: ModrinthVersion) => void;
}

function entryTitle(entry: DependencyEntry): string {
  return (
    entry.resolved?.project.title ||
    entry.dependency.fileName ||
    (entry.dependency.projectId
      ? `项目 ${entry.dependency.projectId}`
      : entry.dependency.versionId
      ? `版本 ${entry.dependency.versionId}`
      : "未知前置")
  );
}

export function RequiredDependencies({
  project,
  version,
  resolve,
  onOpen,
}: Props) {
  const panelId = useId();
  const [open, setOpen] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [entries, setEntries] = useState<DependencyEntry[]>([]);
  const [state, setState] = useState<"idle" | "loading" | "ready" | "error">(
    "idle"
  );
  const [error, setError] = useState("");
  const declared = useMemo(
    () =>
      version.dependencies === undefined
        ? undefined
        : requiredDependencies(version.dependencies, project, version),
    [project, version]
  );

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setState("loading");
    setError("");
    void (async () => {
      try {
        let dependencies = declared;
        if (dependencies === undefined) {
          if (!resolve) throw new Error("前置元数据暂不可用，请重试。");
          const metadata = await resolve({
            projectId: project.projectId || project.slug,
            versionId: version.id,
          });
          if (
            metadata.version?.id !== version.id ||
            metadata.version.dependencies === undefined
          )
            throw new Error("该版本的前置元数据未返回，请重试。");
          dependencies = requiredDependencies(
            metadata.version.dependencies,
            project,
            version
          );
        }
        if (cancelled) return;
        const result: DependencyEntry[] = dependencies.map((dependency) => ({
          dependency,
        }));
        setEntries([...result]);
        let cursor = 0;
        const worker = async () => {
          while (!cancelled && cursor < result.length) {
            const index = cursor++;
            const dependency = result[index].dependency;
            if (!dependency.projectId && !dependency.versionId) continue;
            try {
              if (!resolve)
                throw new Error("前置项目读取接口暂不可用，请重试。");
              const resolved = await resolve(dependency);
              if (
                dependency.versionId &&
                resolved.version?.id !== dependency.versionId
              )
                throw new Error("返回的前置版本与声明不一致，请重试。");
              result[index] = { dependency, resolved };
            } catch (reason) {
              result[index] = {
                dependency,
                error:
                  reason instanceof Error ? reason.message : String(reason),
              };
            }
            if (!cancelled)
              setEntries(
                deduplicateResolvedDependencies(result, project, version)
              );
          }
        };
        await Promise.all(
          Array.from({ length: Math.min(4, result.length) }, worker)
        );
        if (!cancelled) {
          setEntries(deduplicateResolvedDependencies(result, project, version));
          setState("ready");
        }
      } catch (reason) {
        if (!cancelled) {
          setError(reason instanceof Error ? reason.message : String(reason));
          setState("error");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, nonce, declared, project, version, resolve]);

  if (declared?.length === 0) {
    return <p className="pd-deps__none">无必要前置</p>;
  }

  return (
    <div className="pd-deps" data-version-id={version.id}>
      <button
        type="button"
        className="pd-deps__toggle"
        aria-expanded={open}
        aria-controls={panelId}
        aria-label={`查看 ${version.versionNumber || version.name} 的必要前置`}
        onClick={() => setOpen((value) => !value)}
      >
        <span>
          必要前置
          {state === "ready"
            ? ` · ${entries.length}`
            : declared
            ? ` · ${declared.length}`
            : " · 待检测"}
        </span>
        <IconChevronDown aria-hidden="true" />
      </button>
      <Collapse id={panelId} open={open} className="pd-deps__collapse">
        <div className="pd-deps__body" aria-busy={state === "loading"}>
          <p className="pd-deps__hint">
            仅适用于 {version.versionNumber || version.name || version.id}
            ，不会自动安装。
          </p>
          {state === "loading" ? (
            <p className="pd-deps__hint" role="status">
              正在读取必要前置…
            </p>
          ) : null}
          {state === "error" ? (
            <div className="pd-deps__error" role="alert">
              <p>{error} 主资源仍可另存为。</p>
              <Button
                size="sm"
                variant="secondary"
                onClick={() => setNonce((value) => value + 1)}
              >
                重试前置检测
              </Button>
            </div>
          ) : state === "ready" && entries.length === 0 ? (
            <p className="pd-deps__hint">该版本没有声明其他必要前置。</p>
          ) : (
            <ul className="pd-deps__list">
              {entries.map((entry) => {
                const resolved = entry.resolved;
                const title = entryTitle(entry);
                const fileOnly =
                  !entry.dependency.projectId && !entry.dependency.versionId;
                const detail = resolved?.version
                  ? `要求版本 ${
                      resolved.version.versionNumber ||
                      resolved.version.name ||
                      resolved.version.id
                    }`
                  : resolved
                  ? "未指定前置版本，请在详情中选择适配文件。"
                  : fileOnly
                  ? entry.dependency.fileName
                    ? "仅提供文件名，无法定位项目；请手动核对该文件。"
                    : "未提供项目或版本标识，请到项目页核对。"
                  : entry.error || "正在读取项目信息…";
                return (
                  <li
                    key={dependencyKey(entry.dependency)}
                    className="pd-deps__item"
                  >
                    {resolved && onOpen ? (
                      <button
                        type="button"
                        className="pd-deps__project"
                        onClick={() => onOpen(resolved, version)}
                        aria-label={`打开前置 ${title} 的详情`}
                      >
                        <span className="pd-deps__icon" aria-hidden="true">
                          {resolved.project.iconUrl ? (
                            <img src={resolved.project.iconUrl} alt="" />
                          ) : (
                            <CommunityKindIcon
                              kind={dependencyKind(resolved.project)}
                            />
                          )}
                        </span>
                        <span className="pd-deps__text">
                          <span className="pd-deps__title">{title}</span>
                          <span className="pd-deps__meta">{detail}</span>
                        </span>
                        <span className="pd-deps__link">查看详情</span>
                      </button>
                    ) : (
                      <div className="pd-deps__unresolved">
                        <span className="pd-deps__text">
                          <span className="pd-deps__title">{title}</span>
                          <span
                            className="pd-deps__meta"
                            role={entry.error ? "alert" : undefined}
                          >
                            {detail}
                          </span>
                        </span>
                        {entry.error ? (
                          <Button
                            size="sm"
                            variant="secondary"
                            aria-label={`重试读取前置 ${title}`}
                            onClick={() => setNonce((value) => value + 1)}
                          >
                            重试
                          </Button>
                        ) : null}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </Collapse>
    </div>
  );
}
