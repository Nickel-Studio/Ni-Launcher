import type { LaunchProgress } from "./useLauncher";
import "./launch.css";

export interface ProgressBarProps {
  percent: number | null;
  stage: string;
  progress: LaunchProgress | null;
  speedBps: number | null;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function formatSpeed(bps: number | null): string {
  if (!bps || !Number.isFinite(bps) || bps <= 0) return "—";
  return `${formatBytes(bps)}/s`;
}

/** 下载/启动进度条：不确定总量时使用条纹流动的 indeterminate 态。 */
export function ProgressBar({
  percent,
  stage,
  progress,
  speedBps,
}: ProgressBarProps) {
  const indeterminate = percent === null;

  return (
    <div className="progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}>
      <div className={`progress__track${indeterminate ? " is-indeterminate" : ""}`}>
        <div
          className="progress__fill"
          style={indeterminate ? undefined : { width: `${percent}%` }}
        />
      </div>
      <div className="progress__meta">
        <span className="progress__stage">{stage || "处理中..."}</span>
        <span className="progress__right">
          {speedBps ? (
            <span className="progress__speed">{formatSpeed(speedBps)}</span>
          ) : null}
          <span className="progress__percent">
            {percent === null
              ? progress && progress.downloaded > 0
                ? formatBytes(progress.downloaded)
                : "…"
              : `${percent}%`}
          </span>
        </span>
      </div>
      {progress && progress.total > 0 ? (
        <div className="progress__bytes">
          {formatBytes(progress.downloaded)} / {formatBytes(progress.total)}
        </div>
      ) : null}
    </div>
  );
}

export default ProgressBar;
