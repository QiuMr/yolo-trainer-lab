import { useRef, useState } from "react";
import { Check, Info, TriangleAlert, X } from "lucide-react";
import { cx } from "./ui";
import { dismissToast, useToasts, type ToastItem } from "../lib/toast";

/* ─────────────────────────────────────────────────────────────────────────────
   The toast stack.

   Bottom-centre, above the canvas, never over the sidebar. Each toast can be flicked
   away: press and drag down, or click the cross. The drag is tracked rather than
   snap-to-dismiss so the gesture has a physical answer — moving something and watching it
   follow is the whole reason a swipe feels different from a button.
   ───────────────────────────────────────────────────────────────────────────── */

export default function Toaster() {
  const toasts = useToasts();
  if (toasts.length === 0) return null;

  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-6 z-40 flex flex-col items-center gap-2">
      {toasts.map((item) => (
        <Toast key={item.id} item={item} />
      ))}
    </div>
  );
}

function Toast({ item }: { item: ToastItem }) {
  const [offset, setOffset] = useState(0);
  const origin = useRef<number | null>(null);
  const [dragging, setDragging] = useState(false);

  const tone = {
    neutral: "bg-ink text-canvas",
    ok: "bg-ink text-canvas",
    danger: "bg-danger text-canvas",
  }[item.tone];

  const Icon =
    item.tone === "danger" ? TriangleAlert : item.tone === "ok" ? Check : Info;

  return (
    <div
      role="status"
      onPointerDown={(event) => {
        origin.current = event.clientY;
        setDragging(true);
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        if (origin.current === null) return;
        // Downward only: dragging up is not a dismissal, and a toast that flies off in
        // either direction reads as a bug.
        setOffset(Math.max(0, event.clientY - origin.current));
      }}
      onPointerUp={(event) => {
        if (origin.current === null) return;
        const distance = event.clientY - origin.current;
        origin.current = null;
        setDragging(false);
        if (distance > 28) dismissToast(item.id);
        else setOffset(0);
      }}
      onPointerCancel={() => {
        origin.current = null;
        setDragging(false);
        setOffset(0);
      }}
      style={{
        transform: `translateY(${offset}px)`,
        opacity: 1 - Math.min(offset / 140, 0.65),
        // No transition while the finger is down: the toast must track the pointer exactly.
        // Letting go hands control back to the spring, which is what "snap back" means.
        transition: dragging
          ? "none"
          : "transform 220ms var(--ease-spring), opacity 220ms linear",
      }}
      className={cx(
        "animate-toast-in pointer-events-auto flex h-10 max-w-[min(520px,80vw)] items-center gap-2.5",
        "rounded-[10px] pr-2 pl-3.5 text-[12.5px] font-semibold",
        "shadow-[0_16px_40px_-20px_rgba(10,10,10,0.6)] select-none",
        dragging ? "cursor-grabbing" : "cursor-grab",
        tone,
      )}
    >
      <Icon size={14} strokeWidth={2.8} className="shrink-0" />
      <span className="truncate">{item.text}</span>
      <button
        type="button"
        aria-label="关闭"
        onClick={() => dismissToast(item.id)}
        className="ml-1 flex h-6 w-6 shrink-0 items-center justify-center rounded-[6px] opacity-55 transition-[opacity,background-color] duration-120 hover:bg-canvas/15 hover:opacity-100"
      >
        <X size={13} strokeWidth={3} />
      </button>
    </div>
  );
}
