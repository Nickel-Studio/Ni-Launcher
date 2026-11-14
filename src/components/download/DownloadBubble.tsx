import {
  downloadOverview,
  isDownloadBubbleVisible,
  useDownloadCenter,
} from "./store";
import { openDownloadDetail } from "./route";
import { IconCheck } from "../ui/icons";
import { Tooltip } from "../ui";
import { IconAlert } from "../resources/icons";
import { IconDownloadTray } from "./icons";
import "./bubbles.css";

/** 波纹只在入口重新出现时播放，不随阶段或路由切换重播；减弱动效下关闭。 */
export function DownloadBubble() {
  const center = useDownloadCenter();
  const active = center.tasks.filter((item) => item.status === "running");
  const waiting = center.queue.length;
  const failed =
    !!center.startError || center.tasks.some((item) => item.status === "error");
  if (!isDownloadBubbleVisible(center)) return null;
  const status =
    active.length > 0 || waiting > 0 ? "running" : failed ? "error" : "done";
  const label =
    active.length > 0 || waiting > 0
      ? `${active.length} 个任务进行中${
          waiting > 0 ? `，${waiting} 个等待中` : ""
        }，查看下载任务`
      : failed
      ? "下载出现错误，查看下载任务"
      : "下载已结束，查看下载任务";
  const level = downloadOverview(center).percent;
  return (
    <div className="dl-overlay" data-state={status}>
      <span
        key={center.entrance}
        className="dl-overlay__ripple"
        aria-hidden="true"
      />
      <Tooltip label={label}>
        <button
          type="button"
          className="dl-bubble"
          data-state={status}
          aria-label={label}
          onClick={openDownloadDetail}
        >
          <span
            className="dl-bubble__water"
            aria-hidden="true"
            style={{ clipPath: `inset(${100 - level}% 0 0 0)` }}
          />
          <span className="dl-bubble__glyph" aria-hidden="true">
            {status === "done" ? (
              <IconCheck />
            ) : status === "error" ? (
              <IconAlert />
            ) : (
              <IconDownloadTray />
            )}
          </span>
          {active.length + waiting > 1 ? (
            <span className="dl-bubble__multiple" aria-hidden="true" />
          ) : null}
        </button>
      </Tooltip>
    </div>
  );
}