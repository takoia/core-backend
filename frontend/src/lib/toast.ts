// Lightweight toast + notification store (no alert() popups anywhere).
import { writable } from "svelte/store";

export type ToastKind = "info" | "success" | "error" | "warn";
export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  ts: number;
}

let seq = 1;
export const toasts = writable<Toast[]>([]); // currently visible
export const history = writable<Toast[]>([]); // bell history

export function toast(message: string, kind: ToastKind = "info", ttl = 4000) {
  const t: Toast = { id: seq++, kind, message, ts: Date.now() };
  toasts.update((list) => [...list, t]);
  history.update((list) => [t, ...list].slice(0, 50));
  if (ttl > 0) {
    setTimeout(() => dismiss(t.id), ttl);
  }
  return t.id;
}

export function dismiss(id: number) {
  toasts.update((list) => list.filter((t) => t.id !== id));
}

// Promise-based confirm modal (replaces window.confirm).
export interface Confirm {
  message: string;
  resolve: (ok: boolean) => void;
}
export const confirmState = writable<Confirm | null>(null);

export function confirmModal(message: string): Promise<boolean> {
  return new Promise((resolve) => {
    confirmState.set({ message, resolve });
  });
}
