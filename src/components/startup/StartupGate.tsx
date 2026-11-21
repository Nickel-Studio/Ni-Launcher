import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "../../lib/settings";
import { PRODUCT_NAME } from "../../lib/brand";
import {
  acceptEula,
  acknowledgeChangelog,
  BUILTIN_CHANGELOG,
  closeAfterRejection,
  finishOnboarding,
  readOnboarding,
  rejectEula,
  REJECTION_SECONDS,
  rejectionSecondsRemaining,
  type OnboardingStatus,
} from "../../lib/onboarding";
import { Button, Checkbox, Modal } from "../ui";
import { InlineNotice } from "../ui/InlineNotice";
import "./startup.css";

interface Props {
  onReady: () => void;
}

const ignoreDismiss = () => undefined;

export function StartupGate({ onReady }: Props) {
  const [status, setStatus] = useState<OnboardingStatus | null>(null);
  const [checked, setChecked] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const [deadline, setDeadline] = useState<number | null>(null);
  const [remaining, setRemaining] = useState(REJECTION_SECONDS);
  const [complete, setComplete] = useState(false);
  const mounted = useRef(false);
  const finishing = useRef(false);
  const closing = useRef(false);
  const actionPending = useRef(false);

  const onReadyRef = useRef(onReady);

  useLayoutEffect(() => {
    onReadyRef.current = onReady;
  }, [onReady]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    let alive = true;
    setError("");
    void readOnboarding()
      .then((current) => {
        if (!alive) return;
        setStatus(current);
        if (current.exiting) {
          setDeadline(
            (value) => value ?? performance.now() + REJECTION_SECONDS * 1000
          );
        }
      })
      .catch((cause: unknown) => {
        if (alive)
          setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      alive = false;
    };
  }, [reload]);

  useEffect(() => {
    if (
      !status ||
      status.needsEula ||
      status.needsChangelog ||
      status.exiting ||
      deadline !== null ||
      finishing.current
    )
      return;
    finishing.current = true;
    void finishOnboarding()
      .then(() => {
        if (!mounted.current) return;
        setComplete(true);
        onReadyRef.current();
      })
      .catch((cause: unknown) => {
        finishing.current = false;
        if (mounted.current)
          setError(cause instanceof Error ? cause.message : String(cause));
      });
  }, [status, deadline]);

  useEffect(() => {
    if (deadline === null) return;
    const tick = () => {
      const seconds = rejectionSecondsRemaining(deadline, performance.now());
      setRemaining(seconds);
      if (seconds === 0 && !closing.current) {
        closing.current = true;
        // The backend owns the exit timer; this also covers an interrupted IPC reply.
        void closeAfterRejection().catch(() => undefined);
      }
    };
    tick();
    const timer = window.setInterval(tick, 100);
    return () => window.clearInterval(timer);
  }, [deadline]);

  async function submit(action: () => Promise<OnboardingStatus>) {
    if (actionPending.current || deadline !== null) return;
    actionPending.current = true;
    setBusy(true);
    setError("");
    try {
      const current = await action();
      if (mounted.current) {
        setStatus(current);
        if (current.exiting) {
          setDeadline(
            (value) => value ?? performance.now() + REJECTION_SECONDS * 1000
          );
        }
      }
    } catch (cause) {
      if (mounted.current)
        setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      actionPending.current = false;
      if (mounted.current) setBusy(false);
    }
  }

  function reject() {
    if (actionPending.current || deadline !== null) return;
    actionPending.current = true;
    setError("");
    setDeadline(performance.now() + REJECTION_SECONDS * 1000);
    void rejectEula().catch(() => undefined);
  }

  async function readTerms() {
    if (!status) return;
    setError("");
    try {
      if (isTauri()) await openUrl(status.eulaUrl);
      else window.open(status.eulaUrl, "_blank", "noopener,noreferrer");
    } catch {
      if (mounted.current) setError("无法打开浏览器，请稍后重试阅读完整协议。");
    }
  }

  if (complete) return null;
  const stage =
    deadline !== null
      ? "exit"
      : !status
      ? "loading"
      : status.needsEula
      ? "eula"
      : status.needsChangelog
      ? "changelog"
      : "finish";
  const title =
    stage === "eula"
      ? "开始之前，请确认 Minecraft EULA"
      : stage === "changelog"
      ? `${PRODUCT_NAME} ${BUILTIN_CHANGELOG.version} 更新说明`
      : stage === "exit"
      ? "无法继续提供服务"
      : "正在准备首次启动";

  return (
    <Modal
      key={stage}
      open
      title={title}
      onClose={ignoreDismiss}
      dismissible={false}
      closeOnBackdrop={false}
      width={stage === "changelog" ? 620 : 540}
      className="startup-modal"
      footer={
        stage === "eula" && status ? (
          <>
            <Button variant="danger" disabled={busy} onClick={reject}>
              拒绝
            </Button>
            <Button
              variant="primary"
              disabled={!checked || busy}
              loading={busy}
              onClick={() => void submit(() => acceptEula(status, checked))}
            >
              同意并签署
            </Button>
          </>
        ) : stage === "changelog" && status ? (
          <Button
            variant="primary"
            loading={busy}
            onClick={() => void submit(() => acknowledgeChangelog(status))}
          >
            我已知晓
          </Button>
        ) : stage === "exit" ? (
          <Button disabled className="startup-exit-button">
            {`正在退出...${remaining}`}
          </Button>
        ) : error ? (
          <Button
            onClick={() => {
              finishing.current = false;
              setStatus(null);
              setReload((value) => value + 1);
            }}
          >
            重试
          </Button>
        ) : undefined
      }
    >
      {stage === "eula" ? (
        <div className="startup-eula">
          <p>
            使用 Minecraft 前，请阅读并同意其最终用户许可协议。{PRODUCT_NAME}{" "}
            是非官方启动器，不代表 Mojang 或 Microsoft。
          </p>
          <div className="startup-eula__summary">
            <span className="startup-eyebrow">MINECRAFT · EULA</span>
            <p>
              请合法获取游戏，遵守内容分发、Mod
              使用和社区安全相关条款。未成年人应在监护人的帮助下阅读。
            </p>
            <Button variant="outline" onClick={() => void readTerms()}>
              阅读 Minecraft EULA 完整协议
            </Button>
          </div>
          <Checkbox
            checked={checked}
            disabled={busy}
            onChange={setChecked}
            label="我已阅读、理解并同意 Minecraft EULA，愿意签署此声明。"
          />
        </div>
      ) : stage === "changelog" ? (
        <div className="startup-changelog">
          {BUILTIN_CHANGELOG.sections.map((section) => (
            <section key={section.title}>
              <h3>{section.title}</h3>
              <ul>
                {section.items.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      ) : stage === "exit" ? (
        <p className="startup-rejection" role="status">
          如果不同意 Minecraft 的 EULA，{PRODUCT_NAME} 无法提供服务。
        </p>
      ) : !error ? (
        <p role="status">正在准备…</p>
      ) : null}
      {error ? (
        <InlineNotice tone="danger" role="alert">
          {error}
        </InlineNotice>
      ) : null}
    </Modal>
  );
}
