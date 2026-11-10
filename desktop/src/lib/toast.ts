import { useSyncExternalStore } from "react";

/* ─────────────────────────────────────────────────────────────────────────────
   Toasts.

   For feedback that has no home on the page: a folder dropped on the window, an action
   that succeeded somewhere the user is not looking. Anything that belongs to a form stays
   inline — a toast for a field error is a message you cannot act on.
   ───────────────────────────────────────────────────────────────────────────── */

export type ToastTone = "neutral" | "ok" | "danger";

export type ToastItem = {
  id: number;
  text: string;
  tone: ToastTone;
};

let items: ToastItem[] = [];
const listeners = new Set<() => void>();
const timers = new Map<number, number>();
let seq = 0;

const emit = () => {
  for (const listener of listeners) listener();
};

export function dismissToast(id: number) {
  const timer = timers.get(id);
  if (timer !== undefined) {
    window.clearTimeout(timer);
    timers.delete(id);
  }
  items = items.filter((item) => item.id !== id);
  emit();
}

export function toast(text: string, tone: ToastTone = "neutral", ttl = 3400) {
  const id = ++seq;
  // Three at a time, oldest out. A stack taller than the window is not feedback.
  items = [...items, { id, text, tone }].slice(-3);
  emit();
  timers.set(
    id,
    window.setTimeout(() => dismissToast(id), ttl),
  );
  return id;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useToasts() {
  return useSyncExternalStore(
    subscribe,
    () => items,
    () => items,
  );
}
