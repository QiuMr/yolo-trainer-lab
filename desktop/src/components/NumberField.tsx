import { useState } from "react";
import { MoveHorizontal } from "lucide-react";
import { clamp, useScrub } from "../lib/gestures";
import { cx } from "./ui";

/* ─────────────────────────────────────────────────────────────────────────────
   A number field you can drag.

   Typing "640" is three keystrokes, a selection and a mental check that you did not leave
   a digit behind. Dragging is one gesture and the value is always valid. So the input stays
   (typing is still the precise tool) and the label row becomes a scrub surface — the same
   split every 3D and video tool has used for decades.

   The value is normalised on blur, not on every keystroke: clamping while someone is
   halfway through typing "100" would turn their "1" into "8" and make the field feel
   broken. Errors are shown as a red border immediately, corrected at commit time.
   ───────────────────────────────────────────────────────────────────────────── */

export default function NumberField({
  label,
  hint,
  value,
  onChange,
  min,
  max,
  step = 1,
  fallback,
  suffix,
}: {
  label: string;
  hint?: string;
  value: string;
  onChange: (next: string) => void;
  min: number;
  max: number;
  step?: number;
  /** Restored by double-clicking the drag handle or entering garbage. */
  fallback: number;
  suffix?: string;
}) {
  const [touched, setTouched] = useState(false);

  const parsed = Number(value);
  const numeric = Number.isFinite(parsed) ? parsed : fallback;
  const invalid = touched && (value.trim() === "" || parsed < min || parsed > max);

  /** Snap to the grid the engine accepts, so a dragged 641 becomes 640 rather than failing
   *  validation two screens later. */
  const normalise = (raw: number) => {
    const snapped = min + Math.round((raw - min) / step) * step;
    return clamp(Math.round(snapped), min, max);
  };

  const commit = () => {
    setTouched(true);
    const next = normalise(numeric);
    onChange(String(next));
  };

  const scrub = useScrub({
    value: clamp(numeric, min, max),
    min,
    max,
    step,
    onChange: (next) => onChange(String(next)),
    onCommit: (next) => onChange(String(normalise(next))),
  });

  return (
    <label className="block">
      <span className="mb-1.5 flex items-center gap-1.5">
        <span className="text-[13px] font-semibold text-ink">{label}</span>
        <span
          {...scrub.handlers}
          onDoubleClick={() => onChange(String(fallback))}
          title="左右拖动调节 · 按住 Shift 加速 · 双击复位"
          aria-hidden
          className={cx(
            "scrub flex h-5 w-5 items-center justify-center rounded-[5px]",
            "transition-colors duration-120",
            scrub.scrubbing
              ? "bg-ink text-canvas"
              : "text-ink-faint/70 hover:bg-surface-sunken hover:text-ink",
          )}
        >
          <MoveHorizontal size={12} strokeWidth={3} />
        </span>
      </span>

      <span className="relative block">
        <input
          value={value}
          onChange={(event) => {
            setTouched(true);
            onChange(event.target.value);
          }}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit();
              event.currentTarget.blur();
            }
          }}
          inputMode="numeric"
          aria-label={label}
          className={cx(
            "tnum h-11 w-full rounded-[10px] border bg-canvas px-3.5 text-[14px] font-semibold",
            "transition-[border-color,box-shadow] duration-120 outline-none",
            suffix && "pr-12",
            scrub.scrubbing
              ? "border-ink shadow-[0_0_0_3px_rgba(10,10,10,0.07)]"
              : invalid
                ? "border-danger/60"
                : "border-line-strong hover:border-ink/40 focus:border-ink",
          )}
        />
        {suffix ? (
          <span className="pointer-events-none absolute top-1/2 right-3.5 -translate-y-1/2 text-[12px] font-semibold text-ink-faint">
            {suffix}
          </span>
        ) : null}
      </span>

      <span className="mt-1.5 block text-[12px] font-medium text-ink-faint">
        {invalid ? (
          <span className="font-semibold text-danger">
            范围 {min}–{max}
            {step > 1 ? `，步长 ${step}` : ""}
          </span>
        ) : (
          hint
        )}
      </span>
    </label>
  );
}
