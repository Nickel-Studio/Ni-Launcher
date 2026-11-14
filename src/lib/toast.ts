import { useSyncExternalStore } from "react";

/** 状态与渲染分离，下载等非组件代码可直接推送消息；计时由 ToastHost 管理。 */
export type ToastTone = "info" | "success" | "error";

export interface ToastMessage {
  id: number;
  text: string;
  tone: ToastTone;
  /** 自动消失前的停留时长（毫秒），按文本长度计算。 */
  duration: number;
}

const MAX_TOASTS = 6;
const MIN_VISIBLE_MS = 2200;
const MAX_VISIBLE_MS = 6000;

const listeners = new Set<() => void>();
let messages: ToastMessage[] = [];
let sequence = 0;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function emit(): void {
  for (const listener of listeners) listener();
}

function normalizeText(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

function durationFor(text: string): number {
  const length = Math.min(23, Math.max(5, text.length));
  return Math.min(MAX_VISIBLE_MS, Math.max(MIN_VISIBLE_MS, 800 + length * 180));
}

/** 按文本去重，避免同一事件从事件流与快照双通道重复提示。 */
export function showToast(text: string, tone: ToastTone = "info"): void {
  const message = normalizeText(text);
  if (!message) return;
  messages = messages.filter((item) => item.text !== message);
  messages = [
    ...messages,
    {
      id: ++sequence,
      text: message,
      tone,
      duration: durationFor(message),
    },
  ].slice(-MAX_TOASTS);
  emit();
}

export function dismissToast(id: number): void {
  const next = messages.filter((item) => item.id !== id);
  if (next.length === messages.length) return;
  messages = next;
  emit();
}

export function useToasts(): ToastMessage[] {
  return useSyncExternalStore(
    subscribe,
    () => messages,
    () => messages
  );
}
