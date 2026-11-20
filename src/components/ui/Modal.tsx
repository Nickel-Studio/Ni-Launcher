import { useId, useLayoutEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { IconButton } from "./IconButton";
import { IconClose } from "./icons";

export interface ModalProps {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  footer?: ReactNode;
  width?: number;
  closeOnBackdrop?: boolean;
  dismissible?: boolean;
  ariaLabel?: string;
  /** 只影响当前弹窗的局部样式。 */
  className?: string;
}

const FOCUSABLE_SELECTOR =
  'a[href], button, input, select, textarea, [tabindex], [contenteditable="true"]';

function focusableElements(dialog: HTMLElement): HTMLElement[] {
  return Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
    .filter(
      (element) =>
        element.tabIndex >= 0 &&
        !element.matches(":disabled") &&
        !element.closest('[inert], [hidden], [aria-hidden="true"]') &&
        element.getClientRects().length > 0 &&
        getComputedStyle(element).visibility !== "hidden"
    )
    .sort(
      (left, right) =>
        (left.tabIndex || Infinity) - (right.tabIndex || Infinity)
    );
}

export function Modal({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  width = 480,
  closeOnBackdrop = true,
  dismissible = true,
  ariaLabel = "对话框",
  className,
}: ModalProps) {
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  const wasOpenRef = useRef(false);
  const triggerRef = useRef<HTMLElement | null>(null);
  // 每次打开前记录入口，避免子项 autoFocus 先把焦点移进弹窗。
  if (open && !wasOpenRef.current) {
    triggerRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
  }
  wasOpenRef.current = open;

  useLayoutEffect(() => {
    closeRef.current = onClose;
  }, [onClose]);
  useLayoutEffect(() => {
    if (!open) return;
    const dialog = dialogRef.current;
    if (!dialog) return;
    const active = document.activeElement;
    const trigger = triggerRef.current;
    const isTopmost = () =>
      Array.from(
        document.querySelectorAll(".ni-modal[aria-modal='true']")
      ).pop() === dialog;
    const focusFirst = () =>
      (focusableElements(dialog)[0] ?? dialog).focus({ preventScroll: true });
    if (!dialog.contains(active)) focusFirst();

    const onKey = (event: KeyboardEvent) => {
      if (!isTopmost() || event.defaultPrevented || event.isComposing) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (dismissible) closeRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const controls = focusableElements(dialog);
      const first = controls[0];
      const last = controls[controls.length - 1];
      const focused = document.activeElement;
      if (!first) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
      } else if (
        event.shiftKey &&
        (focused === first || !controls.includes(focused as HTMLElement))
      ) {
        event.preventDefault();
        last.focus({ preventScroll: true });
      } else if (
        !event.shiftKey &&
        (focused === last || !controls.includes(focused as HTMLElement))
      ) {
        event.preventDefault();
        first.focus({ preventScroll: true });
      }
    };
    const onFocus = (event: FocusEvent) => {
      if (
        isTopmost() &&
        event.target instanceof Node &&
        !dialog.contains(event.target)
      )
        focusFirst();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("focusin", onFocus);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("focusin", onFocus);
      // 等待 DOM 移除；StrictMode 重挂载或焦点交给新弹窗时不抢回入口。
      queueMicrotask(() => {
        if (
          !dialog.isConnected &&
          trigger?.isConnected &&
          (dialog.contains(document.activeElement) ||
            document.activeElement === document.body)
        ) {
          trigger.focus({ preventScroll: true });
        }
      });
    };
  }, [open, dismissible]);

  if (!open) return null;

  return createPortal(
    <div
      className="ni-modal__backdrop"
      onMouseDown={(event) => {
        if (
          dismissible &&
          closeOnBackdrop &&
          event.target === event.currentTarget
        )
          onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={["ni-modal", className].filter(Boolean).join(" ")}
        role="dialog"
        aria-modal="true"
        aria-labelledby={title != null ? titleId : undefined}
        aria-label={title == null ? ariaLabel : undefined}
        aria-describedby={description != null ? descriptionId : undefined}
        tabIndex={-1}
        style={{ maxWidth: width }}
      >
        {(title != null || description != null) && (
          <div className="ni-modal__head">
            <div>
              {title != null ? (
                <div id={titleId} className="ni-modal__title">
                  {title}
                </div>
              ) : null}
              {description != null ? (
                <div id={descriptionId} className="ni-modal__desc">
                  {description}
                </div>
              ) : null}
            </div>
            {dismissible ? (
              <IconButton label="关闭" size="sm" onClick={onClose}>
                <IconClose />
              </IconButton>
            ) : null}
          </div>
        )}
        {children != null ? (
          <div className="ni-modal__body">{children}</div>
        ) : null}
        {footer != null ? <div className="ni-modal__foot">{footer}</div> : null}
      </div>
    </div>,
    document.body
  );
}
