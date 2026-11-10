import { useCallback, useEffect, useRef, useState } from "react";

/* ─────────────────────────────────────────────────────────────────────────────
   Gestures.

   Pointer and wheel gestures that the platform does not give us for free. Every hook
   here is deliberately small and stateless outside its own ref: gestures are the easiest
   place to introduce a listener leak, so the rule is that all listeners are added in the
   same function that removes them, and nothing persists past unmount.

   Two of these are mouse-only by design (scrub, resize) and two work with any pointer
   (swipe, long-press) because the app also runs on touch-screen laptops.
   ───────────────────────────────────────────────────────────────────────────── */

export const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, value));

/* ── Two-finger horizontal swipe ───────────────────────────────────────────────
   Windows precision trackpads deliver a horizontal two-finger pan as `wheel` with a
   non-zero `deltaX`. That is the gesture people already use in every other app to move
   between screens, and it costs no extra UI.

   A naive listener would fire on diagonal scrolls and on a mouse wheel held sideways, so
   the accumulator insists that the swipe is decisively horizontal (1.8× the vertical
   travel before it counts at all) and only commits once it has travelled far enough —
   a single accidental 20px nudge must not change the page.
   ───────────────────────────────────────────────────────────────────────────── */
export function useSwipe({
  onSwipeLeft,
  onSwipeRight,
  threshold = 72,
  cooldown = 420,
}: {
  onSwipeLeft: () => void;
  onSwipeRight: () => void;
  threshold?: number;
  cooldown?: number;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  // Callbacks change identity on every render; keeping them in a ref means the wheel
  // listener can be registered exactly once for the life of the element.
  const latest = useRef({ onSwipeLeft, onSwipeRight });
  latest.current = { onSwipeLeft, onSwipeRight };

  useEffect(() => {
    const node = ref.current;
    if (!node) return;

    let accX = 0;
    let accY = 0;
    let lastAt = 0;
    let lockedUntil = 0;

    const reset = () => {
      accX = 0;
      accY = 0;
    };

    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey) return; // pinch-zoom, not a pan
      const now = performance.now();
      if (now - lastAt > 260) reset(); // a new gesture, not a continuation
      lastAt = now;

      // Never steal a horizontal scroll from something that actually scrolls sideways.
      const target = event.target as HTMLElement | null;
      if (target?.closest("[data-no-swipe]")) return;

      accX += event.deltaX;
      accY += event.deltaY;
      if (Math.abs(accX) < threshold || Math.abs(accX) < Math.abs(accY) * 1.8) return;
      if (now < lockedUntil) return;

      // Read the direction *before* resetting: `reset()` zeroes `accX`, so branching on it
      // afterwards silently sends every swipe the same way. This is why the accumulator is
      // captured into a local first.
      const goingRight = accX > 0;
      lockedUntil = now + cooldown;
      reset();

      if (goingRight) latest.current.onSwipeLeft();
      else latest.current.onSwipeRight();
    };

    node.addEventListener("wheel", onWheel, { passive: true });
    return () => node.removeEventListener("wheel", onWheel);
  }, [threshold, cooldown]);

  return ref;
}

/* ── Long press ────────────────────────────────────────────────────────────────
   Primary use: the collapsed sidebar. Hover tooltips are useless on a touch screen, and
   an icon-only rail is unusable if you cannot find out what the icons mean. A 380ms hold
   shows the same label the expanded rail would have shown.

   Movement past a small slop cancels: the finger is scrolling or dragging, not holding.
   ───────────────────────────────────────────────────────────────────────────── */
export function useLongPress(onLongPress: () => void, delay = 380) {
  const timer = useRef<number | null>(null);
  const origin = useRef({ x: 0, y: 0 });
  const latest = useRef(onLongPress);
  latest.current = onLongPress;

  const clear = useCallback(() => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  useEffect(() => clear, [clear]);

  return {
    onPointerDown: (event: React.PointerEvent) => {
      if (event.pointerType === "mouse" && event.button !== 0) return;
      origin.current = { x: event.clientX, y: event.clientY };
      clear();
      timer.current = window.setTimeout(() => {
        timer.current = null;
        latest.current();
      }, delay);
    },
    onPointerMove: (event: React.PointerEvent) => {
      if (timer.current === null) return;
      const distance = Math.hypot(
        event.clientX - origin.current.x,
        event.clientY - origin.current.y,
      );
      if (distance > 8) clear();
    },
    onPointerUp: clear,
    onPointerLeave: clear,
    onPointerCancel: clear,
    // A long-press that fires must not also raise the platform context menu.
    onContextMenu: (event: React.MouseEvent) => {
      if (timer.current !== null) event.preventDefault();
    },
  };
}

/* ── Scrub ─────────────────────────────────────────────────────────────────────
   Drag a label left/right to change a number. This is how every DCC tool has adjusted
   values for twenty years, and for a parameter like "epochs" it is far faster than
   click → select-all → type.

   Attached to the label, never to the input: a field you cannot select text in is worse
   than a field you cannot drag. Shift multiplies by ten, because the difference between
   100 and 300 epochs should not require 400px of mouse travel.
   ───────────────────────────────────────────────────────────────────────────── */
export function useScrub({
  value,
  min,
  max,
  step = 1,
  pixelsPerStep = 2,
  onChange,
  onCommit,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  pixelsPerStep?: number;
  onChange: (next: number) => void;
  onCommit?: (next: number) => void;
}) {
  const [scrubbing, setScrubbing] = useState(false);
  const latest = useRef({ value, onChange, onCommit });
  latest.current = { value, onChange, onCommit };

  const gesture = useRef({ lastX: 0, accum: 0, start: 0 });

  /**
   * Listeners go on `window`, not on the handle with pointer capture.
   *
   * Pointer capture looks tidier, but it has to be released explicitly and a missed
   * pointerup leaves the field stuck in the "dragging" state — black ring and all — until
   * something else steals focus. Window listeners are self-removing and survive the pointer
   * leaving the app, which is exactly the case that broke capture.
   */
  const onPointerDown = useCallback(
    (event: React.PointerEvent) => {
      if (event.button !== 0) return;
      event.preventDefault();

      gesture.current = {
        lastX: event.clientX,
        accum: 0,
        start: latest.current.value,
      };
      setScrubbing(true);

      const previousCursor = document.body.style.cursor;
      document.body.style.cursor = "ew-resize";
      document.body.style.userSelect = "none";

      const onMove = (move: PointerEvent) => {
        const state = gesture.current;
        state.accum +=
          ((move.clientX - state.lastX) / pixelsPerStep) *
          (move.shiftKey ? 10 : 1);
        state.lastX = move.clientX;

        const next = clamp(
          state.start + Math.round(state.accum) * step,
          min,
          max,
        );
        if (next !== latest.current.value) latest.current.onChange(next);
      };

      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = "";
        setScrubbing(false);

        const next = clamp(
          gesture.current.start + Math.round(gesture.current.accum) * step,
          min,
          max,
        );
        latest.current.onCommit?.(next);
      };

      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    },
    [max, min, pixelsPerStep, step],
  );

  return {
    scrubbing,
    handlers: { onPointerDown },
  };
}

/* ── Sticky bottom ─────────────────────────────────────────────────────────────
   The log panel follows new output, but only while the user is already at the bottom.
   Auto-scrolling a panel someone has deliberately scrolled up in is the classic chat-app
   offence. `atBottom` is returned so the UI can offer an explicit "jump to latest".
   ───────────────────────────────────────────────────────────────────────────── */
export function useAutoFollow<T extends HTMLElement>(deps: unknown[]) {
  const ref = useRef<T | null>(null);
  const [atBottom, setAtBottom] = useState(true);
  const stick = useRef(true);

  const onScroll = useCallback(() => {
    const node = ref.current;
    if (!node) return;
    const distance = node.scrollHeight - node.scrollTop - node.clientHeight;
    stick.current = distance < 32;
    setAtBottom(stick.current);
  }, []);

  useEffect(() => {
    const node = ref.current;
    if (!node || !stick.current) return;
    node.scrollTop = node.scrollHeight;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  const scrollToLatest = useCallback(() => {
    const node = ref.current;
    if (!node) return;
    stick.current = true;
    setAtBottom(true);
    node.scrollTo({ top: node.scrollHeight, behavior: "smooth" });
  }, []);

  return { ref, onScroll, atBottom, scrollToLatest };
}
