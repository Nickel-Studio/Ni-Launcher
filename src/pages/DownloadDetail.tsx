import { useCallback, useEffect, useRef, useState } from "react";
import { Button, IconButton } from "../components/ui";
import { InlineNotice } from "../components/ui/InlineNotice";
import { IconAlert } from "../components/resources/icons";
import { IconCheck, IconClose, IconRefresh } from "../components/ui/icons";
import { IconDownloadTray } from "../components/download/icons";
import {
  cancelDownload,
  cancelQueuedDownload,
  closeDownloadDetail,
  dismissBubble,
  formatSpeed,
  refreshDownloads,
  useDownloadCenter,
  type DownloadState,
  type QueuedDownload,
} from "../components/download";
import {
  clearDownloadStartError,
  connectDownloadCenter,
  dismissDownload,
  downloadOverview,
} from "../components/download/store";
import "./DownloadDetail.css";

/** 复制反馈的停留时长；与气泡完成停留一样属于交互常量，不是样式令牌。 */
const FEEDBACK_LINGER_MS = 2000;

function titleOf(item: DownloadState): string {
  return item.task?.instanceName || item.task?.versionId || "下载任务";
}

function formatOverviewPercent(percent: number): string {
  if (!Number.isFinite(percent) || percent >= 100) return "100%";
  const floored = Math.floor(Math.max(0, percent) * 100) / 100;
  return `${floored.toFixed(2)}%`;
}

/** 复制错误详情：优先异步剪贴板，失败回退 execCommand，再失败给出可执行提示。 */
async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* 剪贴板 API 不可用（非安全上下文等）时回退到 execCommand。 */
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(area);
    return ok;
  } catch {
    return false;
  }
}

/** 运行中任务卡：一行一个「已发生 + 当前」命名阶段，不伪造未来阶段、不造分母。 */
function TaskCard({ item }: { item: DownloadState }) {
  const taskId = item.task?.taskId ?? "";
  const title = titleOf(item);
  const rows =
    item.stages.length > 0
      ? item.stages
      : [{ name: item.stage || "等待状态更新", detail: item.detail }];
  const lastIndex = rows.length - 1;
  const note = item.cancelRequested
    ? "正在取消，等待下载与文件清理结束…"
    : item.message;
  return (
    <article className="dd-card" data-status="running" aria-busy="true">
      <header className="dd-card__head">
        <h2>{title}</h2>
        <IconButton
          className="dd-cancel"
          label={
            item.cancelRequested ? "等待下载与文件清理结束" : `取消 ${title}`
          }
          disabled={item.cancelRequested}
          onClick={() => void cancelDownload(taskId)}
        >
          <IconClose />
        </IconButton>
      </header>
      <ol className="dd-stages">
        {rows.map((stage, index) => {
          const current = index === lastIndex;
          return (
            <li
              key={`${index}:${stage.name}`}
              className="dd-stage"
              data-current={current || undefined}
            >
              <span className="dd-stage__status">
                {!current ? (
                  <>
                    <IconCheck />
                    <span className="nw-sr-only">已完成</span>
                  </>
                ) : item.percent !== null ? (
                  <span className="nw-mono">{Math.floor(item.percent)}%</span>
                ) : (
                  <>
                    <IconRefresh className="dd-stage__spin" />
                    <span className="nw-sr-only">进行中</span>
                  </>
                )}
              </span>
              <span className="dd-stage__name">{stage.name}</span>
            </li>
          );
        })}
      </ol>
      {note ? (
        <p
          className="dd-card__note"
          data-tone={item.cancelRequested ? "warning" : "danger"}
          role="status"
          aria-live="polite"
        >
          {note}
        </p>
      ) : null}
    </article>
  );
}

function FailedCard({ item }: { item: DownloadState }) {
  const taskId = item.task?.taskId ?? "";
  const title = titleOf(item);
  const message = item.message || "下载失败";
  const [feedback, setFeedback] = useState<"" | "copied" | "failed">("");
  const timer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    []
  );
  const handleCopy = useCallback(async () => {
    const ok = await copyText(message);
    setFeedback(ok ? "copied" : "failed");
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(
      () => setFeedback(""),
      FEEDBACK_LINGER_MS
    );
  }, [message]);
  return (
    <article className="dd-card" data-status="error">
      <header className="dd-card__head">
        <h2>{title}</h2>
        <IconButton
          className="dd-cancel"
          label={`移除 ${title}`}
          onClick={() => dismissDownload(taskId)}
        >
          <IconClose />
        </IconButton>
      </header>
      <div className="dd-failure">
        <span className="dd-failure__mark" aria-hidden="true">
          <IconAlert />
        </span>
        <button
          type="button"
          className="dd-failure__text"
          title="单击复制错误详情"
          onClick={() => void handleCopy()}
        >
          {message}
        </button>
      </div>
      <p
        className="dd-failure__feedback"
        data-feedback={feedback || undefined}
        role="status"
        aria-live="polite"
      >
        {feedback === "copied"
          ? "已复制错误详情"
          : feedback === "failed"
          ? "复制失败，请手动选择文本复制"
          : ""}
      </p>
    </article>
  );
}

/** 等待队列卡：队列第 N 位 + 取消（沿用 cancelQueuedDownload，不调后端）。 */
function QueuedCard({
  queued,
  position,
}: {
  queued: QueuedDownload;
  position: number;
}) {
  const name = queued.task.instanceName || queued.task.versionId;
  return (
    <article className="dd-card" data-status="queued">
      <header className="dd-card__head">
        <h2>{name}</h2>
        <IconButton
          className="dd-cancel"
          label={`移除等待任务 ${name}`}
          disabled={queued.starting}
          onClick={() => cancelQueuedDownload(queued.queueId)}
        >
          <IconClose />
        </IconButton>
      </header>
      <ol className="dd-stages">
        <li className="dd-stage" data-current="true">
          <span className="dd-stage__status nw-mono">
            <span className="nw-sr-only">队列第 </span>
            {position}
            <span className="nw-sr-only"> 位</span>
          </span>
          <span className="dd-stage__name">
            {queued.starting ? "正在启动任务" : "等待前一项完成"}
          </span>
        </li>
      </ol>
    </article>
  );
}

/** 任务清空后返回进入前的界面；待处理错误必须保留在页内。 */
export default function DownloadDetail() {
  const center = useDownloadCenter();
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let alive = true;
    // 先取一次最新快照再判断空态：否则初次挂载的初始空列表会把用户立刻踢回去。
    void connectDownloadCenter()
      .then(() => refreshDownloads())
      .finally(() => {
        if (alive) setReady(true);
      });
    return () => {
      alive = false;
    };
  }, []);
  const overview = downloadOverview(center);
  const active = center.tasks.filter((item) => item.status === "running");
  const failed = center.tasks.filter((item) => item.status === "error");
  const total = center.tasks.length + center.queue.length;
  const isEmpty = total === 0;
  const canHideBubble =
    active.length === 0 &&
    center.queue.length === 0 &&
    (center.tasks.length > 0 || Boolean(center.startError)) &&
    !center.bubbleDismissed;
  // 空页且存在错误时，自动返回会被阻止；此时必须留一个明确的返回入口。
  const trappedByError = isEmpty && Boolean(center.error || center.startError);

  useEffect(() => {
    // 状态同步失败或任务启动失败时保留页面，让用户能看到错误；只有真正没有
    // 任何任务、也没有待处理提示时才强制返回。
    if (ready && isEmpty && !center.startError && !center.error) {
      closeDownloadDetail();
    }
  }, [ready, isEmpty, center.startError, center.error]);

  return (
    <div className="dd-page">
      <header className="dd-page__bar">
        <div className="dd-page__title">
          <h1>下载任务</h1>
        </div>
        <div className="dd-page__actions">
          <span className="dd-count">{total} 个任务</span>
          {trappedByError ? (
            <Button variant="ghost" size="sm" onClick={closeDownloadDetail}>
              返回
            </Button>
          ) : null}
          {canHideBubble ? (
            <Button variant="ghost" size="sm" onClick={dismissBubble}>
              隐藏下载入口
            </Button>
          ) : null}
        </div>
      </header>

      <div className="dd-root" data-empty={isEmpty ? "true" : undefined}>
        {!isEmpty ? (
          <aside className="dd-side" aria-label="全局下载统计">
            <div className="dd-overview">
              <div className="dd-stat">
                <span className="dd-stat__label">总进度</span>
                <strong className="dd-stat__value nw-mono">
                  {formatOverviewPercent(overview.percent)}
                </strong>
              </div>
              <div className="dd-stat">
                <span className="dd-stat__label">下载速度</span>
                <strong className="dd-stat__value nw-mono">
                  {formatSpeed(overview.speedBps)}
                </strong>
              </div>
              <div className="dd-stat">
                <span className="dd-stat__label">剩余文件</span>
                <strong className="dd-stat__value nw-mono">
                  {overview.remainingFiles}
                </strong>
              </div>
            </div>
          </aside>
        ) : null}

        <section
          className="dd-main"
          data-empty={isEmpty ? "true" : undefined}
          aria-label="下载任务"
          aria-busy={active.length > 0 ? true : undefined}
        >
          {center.error ? (
            <InlineNotice
              tone="danger"
              icon={<IconAlert />}
              title="下载状态同步失败"
              role="alert"
              action={
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => void refreshDownloads()}
                >
                  重试
                </Button>
              }
            >
              {center.error}
            </InlineNotice>
          ) : null}

          {center.startError ? (
            <InlineNotice
              tone="danger"
              icon={<IconAlert />}
              title="任务启动失败"
              role="alert"
              action={
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={clearDownloadStartError}
                >
                  清除提示
                </Button>
              }
            >
              {center.startError}
            </InlineNotice>
          ) : null}

          {active.map((item) => (
            <TaskCard key={item.task?.taskId} item={item} />
          ))}
          {center.queue.map((queued, index) => (
            <QueuedCard
              key={queued.queueId}
              queued={queued}
              position={index + 1}
            />
          ))}
          {failed.map((item) => (
            <FailedCard key={item.task?.taskId} item={item} />
          ))}

          {isEmpty ? (
            <div className="dd-empty">
              <IconDownloadTray aria-hidden />
              <h2>暂无下载任务</h2>
              <p>开始安装游戏、整合包或资源后，在这里查看实时进度。</p>
            </div>
          ) : null}
        </section>
      </div>
    </div>
  );
}
