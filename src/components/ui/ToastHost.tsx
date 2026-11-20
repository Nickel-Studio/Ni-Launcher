import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { dismissToast, useToasts, type ToastMessage } from "../../lib/toast";
import { IconAlert } from "../resources/icons";
import { IconCheck } from "./icons";
import "./toast.css";

/** 与 CSS 的退场时长保持一致；减弱动效时直接移除。 */
const EXIT_MS = 180;

function ToastItem({ message }: { message: ToastMessage }) {
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    const timer = window.setTimeout(() => setLeaving(true), message.duration);
    return () => window.clearTimeout(timer);
  }, [message.id, message.duration]);

  useEffect(() => {
    if (!leaving) return;
    const reduced = window.matchMedia(
      "(prefers-reduced-motion: reduce)"
    ).matches;
    const timer = window.setTimeout(
      () => dismissToast(message.id),
      reduced ? 0 : EXIT_MS
    );
    return () => window.clearTimeout(timer);
  }, [leaving, message.id]);

  return (
    <div
      className="ni-toast"
      data-tone={message.tone}
      data-leaving={leaving ? "true" : undefined}
      role="status"
      aria-live="polite"
    >
      <span className="ni-toast__icon" aria-hidden="true">
        {message.tone === "success" ? <IconCheck /> : null}
        {message.tone === "error" ? <IconAlert /> : null}
      </span>
      <span className="ni-toast__text">{message.text}</span>
    </div>
  );
}

/** 全局左下角消息条；固定在视口且不响应指针，不遮挡主界面操作。 */
export function ToastHost() {
  const messages = useToasts();
  if (messages.length === 0) return null;
  return createPortal(
    <div className="ni-toast-host" aria-label="通知">
      {messages.map((message) => (
        <ToastItem key={message.id} message={message} />
      ))}
    </div>,
    document.body
  );
}

export default ToastHost;
