import { useEffect, useRef, useState } from "react";

/* ─────────────────────────────────────────────────────────────────────────────
   Motion runtime.

   CSS handles anything that runs on its own (entrances, hovers, the nav pill). This
   file exists for the two things CSS cannot do: numbers that count, and elements that
   must stay mounted long enough to animate *out*.

   No animation library. A tween is twenty lines of requestAnimationFrame, and pulling in
   a 30 kB dependency for two hooks would be the tail wagging the dog.
   ───────────────────────────────────────────────────────────────────────────── */

const REDUCED_QUERY = "(prefers-reduced-motion: reduce)";

export function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia(REDUCED_QUERY).matches;
}

/** Subscribes rather than reading once: the OS setting can flip while the app is open. */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(prefersReducedMotion);

  useEffect(() => {
    if (!window.matchMedia) return;
    const query = window.matchMedia(REDUCED_QUERY);
    const onChange = () => setReduced(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  return reduced;
}

const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

/**
 * Smoothly moves a displayed number toward `target`.
 *
 * Used for losses, epoch counters and elapsed seconds. Without it, a value that jumps from
 * 4573.58 to 4211.02 between two React renders is unreadable — the eye cannot tell whether
 * the number changed or the layout shifted. Counting through the intermediate values makes
 * the direction of the change legible even at a glance.
 *
 * Interrupting a tween restarts from wherever the display currently is, so a burst of
 * engine events (one per epoch, sometimes several per second) never snaps backwards.
 */
export function useTween(target: number, duration = 420): number {
  const reduced = useReducedMotion();
  const [value, setValue] = useState(target);
  const display = useRef(target);
  const frame = useRef<number | null>(null);

  useEffect(() => {
    if (reduced || !Number.isFinite(target)) {
      display.current = target;
      setValue(target);
      return;
    }

    const from = display.current;
    const delta = target - from;
    if (Math.abs(delta) < 1e-9) return;

    const started = performance.now();
    // Long jumps get a little more time, capped: a 5000→0 reset should not take a second.
    const span = Math.min(duration * 1.6, duration + Math.abs(delta) * 2);

    const tick = (now: number) => {
      const t = Math.min(1, (now - started) / span);
      const next = from + delta * easeOutCubic(t);
      display.current = next;
      setValue(next);
      if (t < 1) frame.current = requestAnimationFrame(tick);
      else frame.current = null;
    };

    frame.current = requestAnimationFrame(tick);
    return () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
    };
  }, [target, duration, reduced]);

  return value;
}

/**
 * Keeps an element mounted while it animates out.
 *
 * A dialog that vanishes on the same frame as the state change has no exit at all — it
 * blinks. `phase` lets the caller switch the CSS class and unmount only after the
 * transition has run.
 */
export function usePresence(open: boolean, exitMs = 150) {
  const [present, setPresent] = useState(open);
  const [phase, setPhase] = useState<"in" | "out">(open ? "in" : "out");

  useEffect(() => {
    if (open) {
      setPresent(true);
      // One frame later, so the browser paints the "out" styles first and has something to
      // transition from. Setting both in the same commit produces a pop, not a fade.
      const frame = requestAnimationFrame(() => setPhase("in"));
      return () => cancelAnimationFrame(frame);
    }
    setPhase("out");
    const timer = window.setTimeout(() => setPresent(false), exitMs);
    return () => window.clearTimeout(timer);
  }, [open, exitMs]);

  return { present, phase };
}

/** Formats with a fixed number of decimals, without the "NaN" that toFixed produces. */
export function formatTween(value: number, digits: number) {
  if (!Number.isFinite(value)) return "—";
  return value.toFixed(digits);
}
