/** 所有社区资源整卡进入内部详情，原生 button 支持 Enter / Space。 */
import { memo } from "react";
import { formatDownloads, formatIsoDate } from "./parse";
import { CommunityKindIcon } from "../community/CommunityKindIcon";
import type { ModrinthHit, ModrinthKind } from "./types";

interface Props {
  hit: ModrinthHit;
  kind: ModrinthKind;
  /** 整卡 / 动作按钮的「查看详情」回调（页面层打开独立详情页）。 */
  onOpen: (hit: ModrinthHit, kind: ModrinthKind) => void;
}

function ModrinthResultCardInner({ hit, kind, onOpen }: Props) {
  const date = formatIsoDate(hit.dateModified);

  return (
    <li className="mr-card">
      <button
        type="button"
        className="mr-card__body"
        data-clickable
        onClick={() => onOpen(hit, kind)}
        aria-label={`查看 ${hit.title} 的版本`}
      >
        <span className="mr-card__icon" aria-hidden="true">
          {hit.iconUrl ? (
            <img src={hit.iconUrl} alt="" loading="lazy" />
          ) : (
            <CommunityKindIcon className="mr-card__icon-fallback" kind={kind} />
          )}
        </span>
        <span className="mr-card__text">
          <span className="mr-card__title">
            <span className="mr-card__name">{hit.title}</span>
            {hit.author ? (
              <span className="mr-card__author">by {hit.author}</span>
            ) : null}
          </span>
          {hit.description ? (
            <span className="mr-card__desc">{hit.description}</span>
          ) : null}
          <span className="mr-card__meta">
            <span className="mr-card__downloads">
              {formatDownloads(hit.downloads)} 下载
            </span>
            <span className="mr-card__source">
              {hit.source === "curseforge" ? "CurseForge" : "Modrinth"}
            </span>
            {hit.loaders.length > 0 ? (
              <span className="mr-card__loaders">
                {hit.loaders.join(" / ")}
              </span>
            ) : null}
            {date ? <span className="mr-card__date">更新于 {date}</span> : null}
          </span>
        </span>
      </button>
    </li>
  );
}

export const ModrinthResultCard = memo(ModrinthResultCardInner);
