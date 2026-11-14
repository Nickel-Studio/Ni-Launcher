import { Button, LoadingCard, Modal } from "../ui";
import { ProgressBar } from "./ProgressBar";
import type { LaunchDiagnostic, LaunchProgress } from "./useLauncher";
import "./launch.css";

export interface LaunchPanelProps {
  /** 实例显示名（副标题） */
  instanceName: string;
  stage: string;
  progress: LaunchProgress | null;
  percent: number | null;
  speedBps: number | null;
  onCancel: () => void;
}

/** 运行日志由失败弹窗的日志目录入口提供，准备面板不重复显示。 */
export function LaunchPanel({
  instanceName,
  stage,
  progress,
  percent,
  speedBps,
  onCancel,
}: LaunchPanelProps) {
  return (
    <div className="launch-panel" role="status" aria-live="polite">
      <LoadingCard size="inline" label="正在启动游戏" />
      <p className="launch-panel__name">{instanceName}</p>
      <ProgressBar
        percent={percent}
        stage={stage || "准备中..."}
        progress={progress}
        speedBps={speedBps}
      />
      <Button
        variant="ghost"
        className="launch-panel__cancel"
        onClick={onCancel}
      >
        取消
      </Button>
    </div>
  );
}

export interface LaunchErrorDialogProps {
  open: boolean;
  /** 诊断标题（崩溃分类）或「启动失败」 */
  title: string;
  message: string;
  onClose: () => void;
  /** 打开本次启动的日志目录；未选择实例时不显示。 */
  onOpenLogs?: () => void;
}

/** 启动失败弹窗：替代内联错误条，错误信息只在这里出现一次。 */
export function LaunchErrorDialog({
  open,
  title,
  message,
  onClose,
  onOpenLogs,
}: LaunchErrorDialogProps) {
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      width={460}
      footer={
        <>
          {onOpenLogs ? (
            <Button variant="ghost" onClick={onOpenLogs}>
              打开日志目录
            </Button>
          ) : null}
          <Button variant="primary" onClick={onClose}>
            知道了
          </Button>
        </>
      }
    >
      <p className="launch-error__message">{message}</p>
    </Modal>
  );
}

export interface LaunchDiagnosticInfo {
  title: string;
  message: string;
}

/** 把 launcher 状态收敛成弹窗文案：诊断优先，其次普通错误。 */
export function launchErrorInfo(
  error: string,
  diagnostic: LaunchDiagnostic | null
): LaunchDiagnosticInfo | null {
  if (diagnostic) {
    return {
      title: diagnostic.title || "启动失败",
      message: diagnostic.message,
    };
  }
  if (error.trim()) {
    return { title: "启动失败", message: error };
  }
  return null;
}

export default LaunchPanel;
