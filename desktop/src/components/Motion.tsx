import type { ReactNode } from "react";
import { useTween, formatTween } from "../lib/motion";
import { clamp } from "../lib/gestures";
import { cx } from "./ui";

/* ─────────────────────────────────────────────────────────────────────────────
   Motion primitives.

   Kept separate from `ui.tsx` so that file stays about layout and colour. Everything
   here follows one rule: motion must carry information the static frame cannot. A number
   that counts tells you it changed and by how much; a delay-staggered list tells you the
   order to read it in. Anything that only exists to look busy is not in here.
   ───────────────────────────────────────────────────────────────────────────── */

/**
 * A number that travels to its new value instead of jumping.
 *
 * `null` renders a dash and is not tweened — "no value" is not zero, and animating to a
 * placeholder would imply a measurement happened.
 */
export function AnimatedNumber({
  value,
  digits = 0,
  className,
  placeholder = "—",
}: {
  value: number | null;
  digits?: number;
  className?: string;
  placeholder?: string;
}) {
  const shown = useTween(value ?? 0);
  if (value === null || !Number.isFinite(value)) {
    return <span className={className}>{placeholder}</span>;
  }
  return <span className={cx("tnum", className)}>{formatTween(shown, digits)}</span>;
}

/**
 * A bar that eases to its new width.
 *
 * The width itself is not transitioned in CSS: a percentage transition re-lays-out the
 * whole row sixty times a second. `useTween` animates the number and lets React write one
 * width per frame, which stays on the compositor for the common case.
 */
export function Progress({
  value,
  className,
  tone = "ink",
}: {
  value: number;
  className?: string;
  tone?: "ink" | "ok" | "danger";
}) {
  const shown = clamp(useTween(value, 560), 0, 100);
  const fill = {
    ink: "bg-ink",
    ok: "bg-ok",
    danger: "bg-danger",
  }[tone];

  return (
    <div
      className={cx("h-1.5 overflow-hidden rounded-full bg-line", className)}
      role="progressbar"
      aria-valuenow={Math.round(value)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={cx("h-full rounded-full", fill)}
        style={{ width: `${shown}%` }}
      />
    </div>
  );
}

/**
 * Entrance with a per-item delay.
 *
 * 55ms steps: any tighter and the stagger reads as a slow load; any wider and the last
 * item arrives after the user has already started reading the first.
 */
export function Reveal({
  index = 0,
  step = 55,
  className,
  children,
}: {
  index?: number;
  step?: number;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cx("animate-card-in", className)}
      style={{ animationDelay: `${index * step}ms` }}
    >
      {children}
    </div>
  );
}

/** The "this is running" dot. Two rings, staggered, so it reads as alive, not as loading. */
export function LiveDot({
  tone = "ok",
  pulse = false,
}: {
  tone?: "ok" | "warn" | "danger";
  pulse?: boolean;
}) {
  const color = {
    ok: "bg-ok",
    warn: "bg-warn",
    danger: "bg-danger",
  }[tone];
  return (
    <span
      className={cx(
        "h-1.5 w-1.5 shrink-0 rounded-full",
        color,
        pulse && "animate-pulse-dot",
      )}
    />
  );
}
