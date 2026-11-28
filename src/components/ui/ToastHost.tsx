import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
} from "react";
import { createPortal } from "react-dom";
import { dismissToast, useToasts, type ToastMessage } from "../../lib/toast";
import { IconAlert } from "../resources/icons";
import { IconCheck } from "./icons";
import "./toast.css";

const EXIT_MS = 180;
const COLLAPSE_MS = 180;
const MOTION_QUERY = "(prefers-reduced-motion: reduce)";
const timing = {
  "--toast-exit-duration": `${EXIT_MS}ms`,
  "--toast-collapse-duration": `${COLLAPSE_MS}ms`,
} as CSSProperties;

function subscribeMotion(listener: () => void) {
  const media = window.matchMedia(MOTION_QUERY);
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}

function reducedMotion() {
  return window.matchMedia(MOTION_QUERY).matches;
}

function ToastItem({
  message,
  leaving,
  reduced,
  onExited,
}: {
  message: ToastMessage;
  leaving: boolean;
  reduced: boolean;
  onExited: (id: number) => void;
}) {
  const slot = useRef<HTMLDivElement>(null);
  const [entered, setEntered] = useState(false);

  useLayoutEffect(() => {
    // Establish the collapsed start before paint; transitions can then reverse mid-flight.
    slot.current?.getBoundingClientRect();
    setEntered(true);
  }, []);

  useEffect(() => {
    if (leaving) return;
    const timer = window.setTimeout(
      () => dismissToast(message.id),
      message.duration
    );
    return () => window.clearTimeout(timer);
  }, [leaving, message.id, message.duration]);

  useEffect(() => {
    if (!leaving) return;
    if (reduced) {
      onExited(message.id);
      return;
    }
    // Fallback for disabled/interrupted transitions and background WebViews.
    const timer = window.setTimeout(
      () => onExited(message.id),
      EXIT_MS + COLLAPSE_MS + 50
    );
    return () => window.clearTimeout(timer);
  }, [leaving, reduced, message.id, onExited]);

  return (
    <div
      ref={slot}
      className="ni-toast-slot"
      data-expanded={entered && !leaving}
      data-leaving={leaving || undefined}
      onTransitionEnd={(event) => {
        if (
          leaving &&
          event.target === event.currentTarget &&
          event.propertyName === "grid-template-rows"
        ) {
          onExited(message.id);
        }
      }}
    >
      <div className="ni-toast-slot__clip">
        <div
          className="ni-toast"
          data-id={message.id}
          data-tone={message.tone}
          data-leaving={leaving || undefined}
          role="status"
          aria-live="polite"
          aria-atomic="true"
          aria-hidden={leaving || undefined}
        >
          <span className="ni-toast__icon" aria-hidden="true">
            {message.tone === "success" ? <IconCheck /> : null}
            {message.tone === "error" ? <IconAlert /> : null}
          </span>
          <span className="ni-toast__text">{message.text}</span>
        </div>
      </div>
    </div>
  );
}

/** 全局左下角消息条；固定在视口且不响应指针，不遮挡主界面操作。 */
export function ToastHost() {
  const messages = useToasts();
  const reduced = useSyncExternalStore(subscribeMotion, reducedMotion);
  const [view, setView] = useState({ snapshot: messages, items: messages });
  if (view.snapshot !== messages) {
    const active = new Map(messages.map((message) => [message.text, message]));
    const retainedTexts = new Set(view.items.map((message) => message.text));
    // Retain removed rows for every dismissal path; refresh duplicates in their own slot.
    setView({
      snapshot: messages,
      items: [
        ...view.items.map((message) => active.get(message.text) ?? message),
        ...messages.filter((message) => !retainedTexts.has(message.text)),
      ],
    });
  }
  const onExited = useCallback((id: number) => {
    setView((current) => ({
      ...current,
      items: current.items.filter((message) => message.id !== id),
    }));
  }, []);
  const activeIds = new Set(messages.map((message) => message.id));
  if (view.items.length === 0) return null;
  return createPortal(
    <div className="ni-toast-host" aria-label="通知" style={timing}>
      {view.items.map((message) => (
        <ToastItem
          key={message.text}
          message={message}
          leaving={!activeIds.has(message.id)}
          reduced={reduced}
          onExited={onExited}
        />
      ))}
    </div>,
    document.body
  );
}

export default ToastHost;
