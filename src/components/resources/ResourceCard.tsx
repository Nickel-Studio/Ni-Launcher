import { useCallback, useState } from "react";
import { Button, Card, Tooltip } from "../ui";
import { KIND_LABELS, type ResourceEntry } from "./types";
import { EMPTY_VALUE, formatBytes, formatTimestamp } from "./format";
import { describeOpenError } from "./parse";
import { openContainingFolder } from "./ipc";
import { IconClock, IconFolderOpen, KIND_ICONS } from "./icons";
import "./resources.css";

interface Props {
  entry: ResourceEntry;
  /** 在列表中的序号，用于交错入场动画。 */
  index: number;
}

/** 单条本地资源卡片：真实文件信息 + 「打开所在文件夹」（失败时卡片内提示）。 */
export function ResourceCard({ entry, index }: Props) {
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState("");

  const handleOpen = useCallback(async () => {
    setOpening(true);
    setError("");
    try {
      await openContainingFolder(entry);
    } catch (err) {
      setError(describeOpenError(err));
    } finally {
      setOpening(false);
    }
  }, [entry]);

  const KindIcon = KIND_ICONS[entry.kind];
  const sizeText =
    entry.sizeBytes !== null ? formatBytes(entry.sizeBytes) : EMPTY_VALUE;
  // 目录（存档）没有字节数时只标「文件夹」，同时给出大小时两者并列。
  const sizeLabel = entry.isDir
    ? sizeText === EMPTY_VALUE
      ? "文件夹"
      : `${sizeText} · 文件夹`
    : sizeText;

  return (
    <Card
      padding="none"
      role="listitem"
      className="rs-card ni-reveal"
      style={{ animationDelay: `${Math.min(index, 14) * 40}ms` }}
    >
      <div className="rs-card__head">
        <span className="rs-card__kind">
          <KindIcon aria-hidden />
          {KIND_LABELS[entry.kind]}
        </span>
        <span className="rs-card__size ni-mono">{sizeLabel}</span>
      </div>

      <Tooltip label={entry.path}>
        <h3 className="rs-card__name" tabIndex={0}>
          {entry.name}
        </h3>
      </Tooltip>

      <p className="rs-card__meta">
        <IconClock aria-hidden />
        <span className="ni-mono">{formatTimestamp(entry.modified)}</span>
      </p>

      <div className="rs-card__foot">
        <Button
          size="sm"
          variant="ghost"
          className="rs-card__open"
          iconLeft={<IconFolderOpen />}
          loading={opening}
          onClick={() => {
            void handleOpen();
          }}
        >
          打开所在文件夹
        </Button>
      </div>

      {error ? (
        <p className="rs-card__error" role="alert">
          {error}
        </p>
      ) : null}
    </Card>
  );
}
