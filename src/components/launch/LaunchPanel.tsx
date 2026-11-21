import { useRef, useState } from "react";
import { Button, LoadingCard, Modal } from "../ui";
import { InlineNotice } from "../ui/InlineNotice";
import { classifyFailure } from "../../lib/failure";
import { ProgressBar } from "./ProgressBar";
import type { LaunchDiagnostic, LaunchProgress } from "./useLauncher";
import "./launch.css";

export interface LaunchPanelProps {
  instanceName: string;
  stage: string;
  progress: LaunchProgress | null;
  percent: number | null;
  speedBps: number | null;
  onCancel: () => void;
  cancelRequested?: boolean;
}

export function LaunchPanel({
  instanceName,
  stage,
  progress,
  percent,
  speedBps,
  onCancel,
  cancelRequested = false,
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
        disabled={cancelRequested}
        onClick={onCancel}
      >
        {cancelRequested ? "正在取消..." : "取消启动"}
      </Button>
    </div>
  );
}

export const ERROR_REPORT_GUIDANCE =
  "寻求帮助时，请导出错误报告并发送完整 ZIP 文件，不要只截图此窗口，也不要只发送其中一份日志。";

export interface LaunchErrorDialogProps {
  open: boolean;
  title: string;
  reason?: string;
  message: string;
  onClose: () => void;
  onOpenLogs?: () => void;
  onExportReport?: () => Promise<string | null>;
}

export function LaunchErrorDialog({
  open,
  title,
  reason,
  message,
  onClose,
  onOpenLogs,
  onExportReport,
}: LaunchErrorDialogProps) {
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState("");
  const [exported, setExported] = useState(false);
  const pending = useRef(false);
  const exportReport = async () => {
    if (!onExportReport || pending.current) return;
    pending.current = true;
    setExporting(true);
    setExportError("");
    setExported(false);
    try {
      const path = await onExportReport();
      setExported(path !== null);
    } catch (error) {
      setExportError(error instanceof Error ? error.message : String(error));
    } finally {
      pending.current = false;
      setExporting(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      description="这次启动遇到了一点问题，先看看下面的简略原因。"
      width={560}
      dismissible={!exporting}
      closeOnBackdrop={!exporting}
      className="launch-error"
      footer={
        <>
          {onOpenLogs ? (
            <Button variant="ghost" disabled={exporting} onClick={onOpenLogs}>
              打开日志目录
            </Button>
          ) : null}
          <Button variant="ghost" disabled={exporting} onClick={onClose}>
            知道了
          </Button>
          {onExportReport ? (
            <Button
              variant="primary"
              disabled={exporting}
              onClick={() => void exportReport()}
            >
              {exporting ? "正在导出完整报告..." : "导出错误报告 ZIP"}
            </Button>
          ) : null}
        </>
      }
    >
      <div className="launch-error__reason">
        <span className="launch-error__label">简略原因</span>
        <p>
          {reason || "暂时没有找到明确原因，请导出完整错误报告进一步检查。"}
        </p>
      </div>
      {message ? <p className="launch-error__message">{message}</p> : null}
      <InlineNotice tone="info" title="如何寻求帮助" role="note">
        {ERROR_REPORT_GUIDANCE}
      </InlineNotice>
      {exportError ? (
        <InlineNotice tone="danger" title="报告未能导出" role="alert">
          {exportError}
        </InlineNotice>
      ) : null}
      {exported ? (
        <InlineNotice tone="info" title="完整报告已导出" role="status">
          请将刚保存的 ZIP 文件完整发送给帮助你的人，发送前可检查日志中由 Mod
          写入的个人内容。
        </InlineNotice>
      ) : null}
    </Modal>
  );
}

export interface LaunchDiagnosticInfo {
  title: string;
  reason: string;
  message: string;
}

function briefReason(value: string): string {
  const line =
    value
      .split(/\r?\n/)
      .map((part) => part.trim())
      .find(Boolean) || "";
  const clean = line.replace(/[\u0000-\u001f\u007f]/g, " ");
  const characters = Array.from(clean);
  return characters.length > 240
    ? `${characters.slice(0, 239).join("")}...`
    : clean;
}

export function launchErrorInfo(
  error: string,
  diagnostic: LaunchDiagnostic | null
): LaunchDiagnosticInfo | null {
  if (diagnostic) {
    return {
      title: "游戏意外退出了",
      reason:
        briefReason(diagnostic.summary || diagnostic.title) ||
        "暂时没有找到明确的崩溃原因。",
      message:
        diagnostic.message || "请导出完整错误报告，日志能帮助进一步定位问题。",
    };
  }
  if (error.trim()) {
    const advice = {
      network: "请检查网络连接与下载源，连接恢复后可以再次启动。",
      corrupt: "请在实例设置中补全或校验文件，然后再试一次。",
      argument: "请检查当前实例与 Java 的启动设置，再试一次。",
      illegal: "请检查账号状态、配置及目录的访问权限，再试一次。",
      unknown: "你可以先重试一次；如果问题仍然出现，请导出完整错误报告。",
    };
    return {
      title: "游戏没能顺利启动",
      reason: briefReason(error),
      message: advice[classifyFailure(error)],
    };
  }
  return null;
}

export default LaunchPanel;
