import { memo, type CSSProperties } from "react";
import { IconButton, Tooltip } from "../ui";
import { IconCheck, IconSettings, IconTrash } from "../ui/icons";
import type { InstanceEntry } from "./instancesApi";
import { blockIconSrc, loaderLabel, typeLabel } from "./instanceVisuals";

export interface InstanceCardProps {
  entry: InstanceEntry;
  index: number;
  selected: boolean;
  busy: boolean;
  onSelect: (entry: InstanceEntry) => void;
  onSettings: (entry: InstanceEntry) => void;
  onFavorite: (entry: InstanceEntry) => void;
  onDelete: (entry: InstanceEntry) => void;
}

/**
 * 实例卡：整卡是唯一选择入口，点击即选中并返回首页；右下角收藏 / 删除 / 设置
 * 三个图标按钮互为兄弟，鼠标和键盘都不会误触选择。
 */
export const InstanceCard = memo(function InstanceCard({
  entry,
  index,
  selected,
  busy,
  onSelect,
  onSettings,
  onFavorite,
  onDelete,
}: InstanceCardProps) {
  return (
    <article
      className={`vs-card ni-card${selected ? " is-selected" : ""}${
        entry.hasJson ? "" : " is-broken"
      }`}
      style={{ "--vs-index": index } as CSSProperties}
      data-instance-id={entry.id}
      data-selected={selected ? "true" : "false"}
    >
      <button
        type="button"
        className="vs-card__select"
        aria-label={`选择实例 ${entry.displayName}`}
        aria-pressed={selected}
        disabled={busy || !entry.hasJson}
        onClick={() => onSelect(entry)}
      >
        <img
          className="vs-card__icon"
          src={blockIconSrc(entry.icon)}
          alt=""
          width={40}
          height={40}
          draggable={false}
        />
        <span className="vs-card__head">
          <Tooltip label={entry.displayName}>
            <span className="vs-card__name">{entry.displayName}</span>
          </Tooltip>
          <span className="vs-card__subtitle">
            {entry.minecraftVersion || typeLabel(entry.type)} ·{" "}
            {loaderLabel(entry.loader)}
            {entry.loaderVersion ? ` ${entry.loaderVersion}` : ""}
            {entry.favorite ? " · 已收藏" : ""}
          </span>
        </span>
        {selected ? (
          <span className="vs-card__selected" aria-label="当前启动实例">
            <IconCheck />
          </span>
        ) : null}
      </button>
      <div className="vs-card__foot">
        {!entry.hasJson ? (
          <span className="vs-card__broken">版本文件缺失</span>
        ) : null}
        <div className="vs-card__tools">
          <IconButton
            label={entry.favorite ? "取消收藏" : "收藏实例"}
            size="sm"
            disabled={busy}
            aria-pressed={entry.favorite}
            className={`vs-card__favorite${
              entry.favorite ? " is-favorite" : ""
            }`}
            onClick={() => onFavorite(entry)}
          >
            <svg
              viewBox="0 0 24 24"
              fill={entry.favorite ? "currentColor" : "none"}
              stroke="currentColor"
              strokeWidth="1.75"
              aria-hidden
            >
              <path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z" />
            </svg>
          </IconButton>
          <IconButton
            label={`删除实例 ${entry.displayName}`}
            size="sm"
            disabled={busy}
            onClick={() => onDelete(entry)}
          >
            <IconTrash />
          </IconButton>
          <IconButton
            label={`实例设置 ${entry.displayName}`}
            size="sm"
            disabled={busy}
            onClick={() => onSettings(entry)}
          >
            <IconSettings />
          </IconButton>
        </div>
      </div>
    </article>
  );
});
