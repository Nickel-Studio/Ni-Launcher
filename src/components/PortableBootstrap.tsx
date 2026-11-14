import { useEffect, useState } from "react";
import {
  initializePortable,
  readPortableStatus,
  startPortableImport,
  type PortableStatus,
} from "../lib/portable";
import { useDownloadCenter, cancelDownload } from "./download/store";
import { describeInstallError } from "./download/parse";
import { Button, Modal } from "./ui";

export function PortableBootstrap() {
  const center = useDownloadCenter();
  const [status, setStatus] = useState<PortableStatus | null>(null);
  const [error, setError] = useState("");
  const [starting, setStarting] = useState(false);
  const [closed, setClosed] = useState(false);
  const task = center.tasks.find(
    (item) => item.task?.taskId === status?.taskId
  );
  const terminal = task?.status === "error" || task?.status === "cancelled";

  useEffect(() => {
    let alive = true;
    void initializePortable()
      .then((value) => {
        if (alive) setStatus(value);
      })
      .catch((cause: unknown) => {
        if (alive) setError(describeInstallError(cause));
      });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    if (terminal) setClosed(false);
  }, [terminal, status?.taskId]);

  async function retry() {
    setStarting(true);
    setError("");
    try {
      const current = await readPortableStatus();
      if (!current) {
        setStatus(null);
        return;
      }
      setStatus(
        current.state === "installed"
          ? current
          : await startPortableImport(current)
      );
    } catch (cause) {
      setError(describeInstallError(cause));
    } finally {
      setStarting(false);
    }
  }

  const running =
    Boolean(status?.taskId) && !terminal && task?.status !== "done";
  const pending = status?.state === "pending" && !status.taskId;
  const open = !closed && (Boolean(error) || terminal || running || pending);
  return (
    <Modal
      open={open}
      title={
        terminal || error || pending
          ? "便携整合包导入未完成"
          : "正在准备便携整合包"
      }
      description={status?.name}
      onClose={() => setClosed(true)}
      closeOnBackdrop={false}
      footer={
        <>
          <Button onClick={() => setClosed(true)}>
            {running ? "后台继续" : "稍后再试"}
          </Button>
          {running ? (
            <Button
              variant="danger"
              disabled={task?.cancelRequested || !task}
              onClick={() => {
                if (status?.taskId) void cancelDownload(status.taskId);
              }}
            >
              {task?.cancelRequested ? "正在取消" : "取消导入"}
            </Button>
          ) : (
            <Button
              variant="primary"
              loading={starting}
              onClick={() => void retry()}
            >
              重试导入
            </Button>
          )}
        </>
      }
    >
      {error || terminal || pending ? (
        <p role="alert">
          {error || task?.message || "导入尚未完成，点击重试可重新导入。"}
        </p>
      ) : (
        <p role="status">
          {task?.stage || "等待导入任务开始"}
          {task?.detail ? ` · ${task.detail}` : ""}
        </p>
      )}
      {status ? <p>游戏目录：{status.gameDirectory}</p> : null}
    </Modal>
  );
}
