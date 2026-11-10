import { useEffect, useMemo, useRef, useState } from "react";
import type { LucideIcon } from "lucide-react";
import { CornerDownLeft, Search } from "lucide-react";
import { cx, Kbd } from "./ui";
import { usePresence } from "../lib/motion";

/* ─────────────────────────────────────────────────────────────────────────────
   Command palette.

   Ctrl+K is a keyboard gesture, not a feature: it is the fastest way to move around an
   app whose navigation is a rail of four icons, and it makes every action reachable
   without a mouse. It also happens to be the honest preview of the AI control surface —
   every row is a named command that can later be triggered by a prompt instead of a
   keypress.

   No modal library. The whole thing is a fixed overlay, a filter and an index, and it
   needs to own its focus behaviour anyway.
   ───────────────────────────────────────────────────────────────────────────── */

export type PaletteAction = {
  id: string;
  label: string;
  /** Secondary text; also searched, so "参数" finds 训练. */
  hint?: string;
  icon: LucideIcon;
  keys?: string;
  run: () => void;
};

export default function CommandPalette({
  open,
  onClose,
  actions,
}: {
  open: boolean;
  onClose: () => void;
  actions: PaletteAction[];
}) {
  const { present, phase } = usePresence(open, 140);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return actions;
    return actions.filter((action) =>
      `${action.label} ${action.hint ?? ""}`.toLowerCase().includes(needle),
    );
  }, [actions, query]);

  // Reopening always starts from a clean slate: a stale query from last time hides the
  // very commands the user opened the palette to find.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setCursor(0);
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open]);

  useEffect(() => {
    setCursor((value) => Math.min(value, Math.max(0, matches.length - 1)));
  }, [matches.length]);

  // Keep the highlighted row in view when it is moved with the arrow keys. `nearest` scrolls
  // the minimum distance, which avoids the list jumping to centre on every keypress.
  useEffect(() => {
    const node = listRef.current?.children[cursor] as HTMLElement | undefined;
    node?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  if (!present) return null;

  const run = (action: PaletteAction) => {
    onClose();
    // Let the overlay start its exit before the action moves the page underneath it.
    window.setTimeout(action.run, 0);
  };

  return (
    <div
      className={cx(
        "fixed inset-0 z-50 flex items-start justify-center px-6 pt-[16vh]",
        "transition-opacity duration-140",
        phase === "in" ? "opacity-100" : "opacity-0",
      )}
    >
      <div
        className="absolute inset-0 bg-ink/25 backdrop-blur-[2px]"
        onClick={onClose}
      />

      <div
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
        className={cx(
          "relative w-full max-w-[560px] overflow-hidden rounded-[14px] bg-canvas",
          "shadow-[0_24px_60px_-20px_rgba(10,10,10,0.45)]",
          phase === "in" ? "animate-pop-in" : "opacity-0",
        )}
      >
        <div className="flex h-13 items-center gap-2.5 px-4">
          <Search size={16} strokeWidth={2.6} className="shrink-0 text-ink-faint" />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setCursor((value) => (value + 1) % Math.max(1, matches.length));
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setCursor(
                  (value) =>
                    (value - 1 + Math.max(1, matches.length)) %
                    Math.max(1, matches.length),
                );
              } else if (event.key === "Enter") {
                event.preventDefault();
                const action = matches[cursor];
                if (action) run(action);
              } else if (event.key === "Escape") {
                event.preventDefault();
                onClose();
              }
            }}
            placeholder="搜索页面或操作…"
            className="h-full w-full bg-transparent text-[14.5px] font-semibold placeholder:font-medium placeholder:text-ink-faint focus:outline-none"
          />
          <Kbd>esc</Kbd>
        </div>

        <div
          ref={listRef}
          data-no-swipe
          className="max-h-[320px] overflow-y-auto px-2 pb-2"
          role="listbox"
        >
          {matches.length === 0 ? (
            <div className="px-3 py-8 text-center text-[13px] font-medium text-ink-faint">
              没有匹配的操作
            </div>
          ) : (
            matches.map((action, index) => {
              const Icon = action.icon;
              const active = index === cursor;
              return (
                <button
                  key={action.id}
                  type="button"
                  role="option"
                  aria-selected={active}
                  onMouseEnter={() => setCursor(index)}
                  onClick={() => run(action)}
                  className={cx(
                    "flex h-11 w-full items-center gap-3 rounded-[8px] px-3 text-left",
                    "transition-colors duration-120 ease-out",
                    active
                      ? "bg-ink text-canvas"
                      : "text-ink hover:bg-surface-sunken",
                  )}
                >
                  <Icon
                    size={16}
                    strokeWidth={2.4}
                    className={cx("shrink-0", active ? "text-canvas" : "text-ink-soft")}
                  />
                  <span className="text-[13.5px] font-semibold">{action.label}</span>
                  {action.hint ? (
                    <span
                      className={cx(
                        "ml-auto text-[11.5px] font-medium",
                        active ? "text-canvas/60" : "text-ink-faint",
                      )}
                    >
                      {action.hint}
                    </span>
                  ) : null}
                  {active ? (
                    <CornerDownLeft
                      size={13}
                      strokeWidth={2.6}
                      className="ml-3 shrink-0 text-canvas/60"
                    />
                  ) : null}
                </button>
              );
            })
          )}
        </div>

        <div className="flex items-center gap-4 px-4 pt-1 pb-3 text-[11px] font-semibold text-ink-faint">
          <span className="flex items-center gap-1.5">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> 选择
          </span>
          <span className="flex items-center gap-1.5">
            <Kbd>↵</Kbd> 执行
          </span>
          <span className="ml-auto">{matches.length} 项</span>
        </div>
      </div>
    </div>
  );
}
