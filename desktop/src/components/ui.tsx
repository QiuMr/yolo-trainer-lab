import type { LucideIcon } from "lucide-react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode } from "react";
import { AnimatedNumber, LiveDot } from "./Motion";

/* ─────────────────────────────────────────────────────────────────────────────
   Shared primitives.

   Two rules hold across every control: it carries an icon when it has an action,
   and it uses one of three radii (14 card / 10 control / 8 chip). Nothing here
   invents a new colour — the palette is white, ink, and three status hues.
   ───────────────────────────────────────────────────────────────────────────── */

function cx(...parts: Array<string | false | null | undefined>) {
  return parts.filter(Boolean).join(" ");
}

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const BUTTON_VARIANTS: Record<ButtonVariant, string> = {
  primary:
    "bg-ink text-canvas hover:bg-ink/85 active:bg-ink disabled:bg-ink/30",
  secondary:
    "bg-canvas text-ink border border-line-strong hover:border-ink hover:bg-surface active:bg-surface-sunken disabled:text-ink-faint disabled:hover:border-line-strong disabled:hover:bg-canvas",
  ghost:
    "bg-transparent text-ink-soft hover:bg-surface-sunken hover:text-ink disabled:text-ink-faint disabled:hover:bg-transparent",
  danger:
    "bg-canvas text-danger border border-danger/35 hover:bg-danger hover:text-canvas active:bg-danger/90",
};

export function Button({
  icon: Icon,
  children,
  variant = "primary",
  size = "md",
  loading = false,
  className,
  ...rest
}: {
  icon?: LucideIcon;
  children?: ReactNode;
  variant?: ButtonVariant;
  size?: "sm" | "md" | "lg";
  loading?: boolean;
} & ButtonHTMLAttributes<HTMLButtonElement>) {
  const sizes = {
    sm: "h-9 px-3 text-[13px] gap-1.5",
    md: "h-11 px-4 text-[14px] gap-2",
    lg: "h-12 px-5 text-[15px] gap-2.5",
  } as const;

  return (
    <button
      {...rest}
      disabled={rest.disabled || loading}
      className={cx(
        "inline-flex items-center justify-center rounded-[10px] font-semibold",
        // The press feedback is a scale, not a colour change: it reads on both the black and the
        // white-outline variant, and it is the only motion a control should ever need.
        "transition-[background-color,color,border-color,transform] duration-120 ease-out",
        "active:scale-[0.985] disabled:active:scale-100",
        "select-none whitespace-nowrap disabled:cursor-not-allowed",
        sizes[size],
        BUTTON_VARIANTS[variant],
        className,
      )}
    >
      {Icon ? (
        <Icon
          size={size === "sm" ? 15 : 17}
          strokeWidth={2.3}
          className={loading ? "animate-spin" : undefined}
        />
      ) : null}
      {children}
    </button>
  );
}

export function Card({
  children,
  className,
  padded = true,
  enter,
  interactive = false,
}: {
  children: ReactNode;
  className?: string;
  padded?: boolean;
  /** Stagger index for the entrance. Omit to render without motion (e.g. a card that
   *  re-renders constantly should not replay an entrance on every data tick). */
  enter?: number;
  /** Adds the pick-up gesture: a 1px lift on hover, a squash on press. For cards whose
   *  whole surface is clickable; a read-only card must not pretend to be a target. */
  interactive?: boolean;
}) {
  return (
    <div
      className={cx(
        "rounded-[14px] border border-line bg-canvas",
        padded && "p-6",
        interactive && "lift cursor-pointer",
        enter !== undefined && "animate-card-in",
        className,
      )}
      style={enter === undefined ? undefined : { animationDelay: `${enter * 55}ms` }}
    >
      {children}
    </div>
  );
}

export function SectionTitle({
  icon: Icon,
  title,
  hint,
  action,
  className,
}: {
  icon?: LucideIcon;
  title: string;
  hint?: string;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cx("mb-5 flex items-start justify-between gap-4", className)}>
      <div className="flex items-start gap-3">
        {Icon ? (
          <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-[8px] bg-surface-sunken">
            <Icon size={16} strokeWidth={2.4} />
          </span>
        ) : null}
        <div>
          <h3 className="text-[15px] font-bold tracking-tight">{title}</h3>
          {hint ? (
            <p className="mt-1 text-[13px] leading-relaxed font-medium text-ink-soft">
              {hint}
            </p>
          ) : null}
        </div>
      </div>
      {action}
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-[13px] font-semibold text-ink">
        {label}
      </span>
      {children}
      {hint ? (
        <span className="mt-1.5 block text-[12px] font-medium text-ink-faint">
          {hint}
        </span>
      ) : null}
    </label>
  );
}

export function TextInput(props: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...props}
      className={cx(
        "h-11 w-full rounded-[10px] border border-line-strong bg-canvas px-3.5",
        "text-[14px] font-semibold placeholder:font-medium placeholder:text-ink-faint",
        "transition-colors hover:border-ink/40 focus:border-ink focus:outline-none",
        props.className,
      )}
    />
  );
}

export function Badge({
  children,
  tone = "neutral",
  icon: Icon,
  pulse = false,
}: {
  children: ReactNode;
  tone?: "neutral" | "ok" | "warn" | "danger";
  icon?: LucideIcon;
  /** For a live state only ("training now"). A pulsing badge on a static state is a lie. */
  pulse?: boolean;
}) {
  const tones = {
    neutral: "bg-surface-sunken text-ink-soft",
    ok: "bg-ok/10 text-ok",
    warn: "bg-warn/10 text-warn",
    danger: "bg-danger/10 text-danger",
  } as const;
  return (
    <span
      className={cx(
        "inline-flex items-center gap-1.5 rounded-[8px] px-2.5 py-1 text-[12px] font-bold",
        tones[tone],
      )}
    >
      {pulse ? (
        <LiveDot tone={tone === "neutral" ? "ok" : tone} pulse />
      ) : Icon ? (
        <Icon size={13} strokeWidth={2.6} />
      ) : null}
      {children}
    </span>
  );
}

export function Stat({
  label,
  value,
  unit,
  icon: Icon,
  digits = 0,
  placeholder,
  enter,
}: {
  label: string;
  value: ReactNode | number | null;
  unit?: string;
  icon?: LucideIcon;
  /** Only applies when `value` is a number: decimals shown while it counts. */
  digits?: number;
  placeholder?: string;
  /** Stagger index for the entrance, same convention as `Card`. */
  enter?: number;
}) {
  return (
    <div
      className={cx(
        "rounded-[14px] border border-line bg-canvas p-5",
        enter !== undefined && "animate-card-in",
      )}
      style={enter === undefined ? undefined : { animationDelay: `${enter * 55}ms` }}
    >
      <div className="flex items-center gap-2 text-[12px] font-bold tracking-wide text-ink-faint uppercase">
        {Icon ? <Icon size={13} strokeWidth={2.6} /> : null}
        {label}
      </div>
      <div className="mt-2 flex items-baseline gap-1.5">
        <span className="text-[26px] leading-none font-bold tracking-tight">
          {typeof value === "number" || value === null ? (
            <AnimatedNumber
              value={value as number | null}
              digits={digits}
              placeholder={placeholder}
            />
          ) : (
            value
          )}
        </span>
        {unit ? (
          <span className="text-[13px] font-semibold text-ink-faint">{unit}</span>
        ) : null}
      </div>
    </div>
  );
}

/** A keycap. Used in tooltips and the command palette so a shortcut is discoverable
 *  without a manual — the label is the documentation. */
export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-5 min-w-5 items-center justify-center rounded-[5px] border border-line-strong bg-surface px-1.5 font-sans text-[10.5px] font-bold text-ink-soft">
      {children}
    </kbd>
  );
}

export { cx };
