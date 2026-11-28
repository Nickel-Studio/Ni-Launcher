import { AnimatedSwap } from "../ui/AnimatedSwap";
import { LaunchReveal } from "./LaunchTransition";
import type { LaunchPhase, LaunchProgress } from "./useLauncher";
import "./launch.css";

export interface ProgressBarProps {
  phase: LaunchPhase;
  percent: number | null;
  stage: string;
  progress: LaunchProgress | null;
  speedBps: number | null;
  cancelRequested: boolean;
}

function formatSpeed(bps: number): string {
  const units = ["B", "KB", "MB", "GB"];
  let value = Math.max(0, bps);
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value >= 100 || i === 0 ? 0 : 1)} ${units[i]}/s`;
}

export function ProgressBar({
  phase,
  percent,
  stage,
  progress,
  speedBps,
  cancelRequested,
}: ProgressBarProps) {
  const measurable = !cancelRequested && (phase === "checking" || phase === "downloading");
  const showPercent = measurable && percent !== null;
  const showSpeed = measurable && phase === "downloading" && speedBps !== null;
  const showCount = measurable && progress !== null && (progress.total > 0 || progress.downloaded > 0);
  const message = cancelRequested ? "正在取消，请稍候..." : stage || "准备中...";

  return (
    <div className="progress" data-phase={cancelRequested ? "cancelling" : phase}>
      <div className="progress__stage" role="status" aria-live="polite" title={message}>
        <AnimatedSwap swapKey={message}>{message}</AnimatedSwap>
      </div>
      <LaunchReveal visible={measurable} className="progress__bar-reveal">
        <div
          className={`progress__track${percent === null ? " is-indeterminate" : ""}`}
          role="progressbar"
          aria-label={phase === "checking" ? "文件校验进度" : "文件下载进度"}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent ?? undefined}
        >
          <div className="progress__fill" style={percent === null ? undefined : { width: `${percent}%` }} />
        </div>
      </LaunchReveal>
      <LaunchReveal visible={showPercent} className="progress__percent-reveal">
        <div className="progress__row">
          <span>当前步骤进度</span>
          <span className="progress__percent">{percent}%</span>
        </div>
      </LaunchReveal>
      <LaunchReveal visible={showCount} className="progress__count-reveal">
        <div className="progress__row">
          <span>{phase === "checking" ? "已校验文件" : "已下载文件"}</span>
          <span className="progress__count">
            {progress?.downloaded.toLocaleString()}
            {progress && progress.total > 0 ? ` / ${progress.total.toLocaleString()}` : ""}
          </span>
        </div>
      </LaunchReveal>
      <LaunchReveal visible={showSpeed} className="progress__speed-reveal">
        <div className="progress__row">
          <span>下载速度</span>
          <span className="progress__speed">{formatSpeed(speedBps ?? 0)}</span>
        </div>
      </LaunchReveal>
    </div>
  );
}

export default ProgressBar;
