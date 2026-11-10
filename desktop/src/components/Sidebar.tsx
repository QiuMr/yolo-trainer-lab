import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { Activity, FolderTree, Play, Settings } from "lucide-react";
import { cx } from "./ui";
import { useLongPress } from "../lib/gestures";

/* ─────────────────────────────────────────────────────────────────────────────
   Sidebar: collapsible, resizable by dragging its right edge.

   Two independent states on purpose — `collapsed` is a one-click mode switch, `width`
   is a gesture that only applies while expanded. Folding them into one number would
   mean collapsing and expanding again loses the width the user dragged to.

   No brand block at the top: the collapse toggle lives in the title bar's corner and
   sits on the same vertical line as these icons, so a logo above them only pushed the
   navigation down for decoration. Settings is pinned to the bottom edge where a
   preferences entry belongs, and carries the engine status as its secondary text —
   that keeps one line of chrome instead of a separate status card.

   ── The selection pill ──────────────────────────────────────────────────────
   One black rectangle slides between rows instead of each row lighting up on its own.
   That single object is what makes the rail feel like a physical control: the eye
   follows the movement and knows exactly where the selection went, which a cross-fade
   between two identical black backgrounds cannot communicate.

   Measuring is done by comparing client rects against the column, NOT by `offsetTop`.
   `offsetTop` is relative to the nearest positioned ancestor — and each row's parent
   (`<nav>` for the main list, the `mt-auto` block for the pinned settings row) is itself
   `relative`, so offsetTop restarts at 0 in every group. That made the pill sit 6px high
   (it ignored the column's top padding) and, worse, made it jump back to the top when the
   pinned row was selected: the row was flagged active and turned white while the pill was
   still on the first entry, so "设置" looked like it had vanished and 数据集 looked selected.
   ───────────────────────────────────────────────────────────────────────────── */

export type PageId = "dataset" | "train" | "monitor" | "settings";

export type NavItem = {
  id: PageId;
  label: string;
  icon: typeof FolderTree;
  hint: string;
  /** Rendered at the bottom of the rail rather than in the main list. */
  pinned?: boolean;
};

// Order matters: Ctrl+1..4 follows this array, and swipe direction follows the order.
export const NAV: NavItem[] = [
  { id: "dataset", label: "数据集", icon: FolderTree, hint: "扫描与校验" },
  { id: "train", label: "训练", icon: Play, hint: "参数与启动" },
  { id: "monitor", label: "监控", icon: Activity, hint: "曲线与日志" },
  {
    id: "settings",
    label: "设置",
    icon: Settings,
    hint: "运行环境",
    pinned: true,
  },
];

const COLLAPSED_WIDTH = 64;
const MIN_WIDTH = 184;
const MAX_WIDTH = 340;
const DEFAULT_WIDTH = 236;
const KEY_COLLAPSED = "yolo.sidebar.collapsed";
const KEY_WIDTH = "yolo.sidebar.width";

/** `?sidebar=collapsed` forces the folded rail. Same reason as App's `?page=`: this state is
 *  otherwise reachable only by clicking, and screenshots need it directly. A forced state is
 *  never written back to storage — a QA switch must not become the user's preference. */
const FORCED_COLLAPSED =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("sidebar") === "collapsed";

const clampWidth = (value: number) =>
  Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(value)));

export function useSidebar() {
  const [collapsed, setCollapsed] = useState(() => {
    try {
      if (FORCED_COLLAPSED) return true;
      return localStorage.getItem(KEY_COLLAPSED) === "1";
    } catch {
      return false;
    }
  });
  const [width, setWidth] = useState(() => {
    try {
      const stored = Number(localStorage.getItem(KEY_WIDTH));
      return Number.isFinite(stored) && stored > 0
        ? clampWidth(stored)
        : DEFAULT_WIDTH;
    } catch {
      return DEFAULT_WIDTH;
    }
  });
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    if (FORCED_COLLAPSED) return;
    try {
      localStorage.setItem(KEY_COLLAPSED, collapsed ? "1" : "0");
    } catch {
      /* private mode / quota: a lost preference is not worth crashing over */
    }
  }, [collapsed]);

  useEffect(() => {
    try {
      localStorage.setItem(KEY_WIDTH, String(width));
    } catch {
      /* see above */
    }
  }, [width]);

  /** Pointer gesture: drag the right edge to resize. Listener lives on the window so the
   *  pointer may leave the 6px handle without the drag dying mid-way. */
  const startResize = useCallback(
    (event: React.PointerEvent) => {
      if (event.button !== 0) return;
      event.preventDefault();
      const startX = event.clientX;
      const startWidth = width;
      setDragging(true);
      // The cursor has to be pinned for the whole drag: passing over the content area would
      // otherwise show a text caret halfway through the gesture.
      const previousCursor = document.body.style.cursor;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";

      const onMove = (move: PointerEvent) => {
        setWidth(clampWidth(startWidth + move.clientX - startX));
      };
      const onUp = () => {
        setDragging(false);
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = "";
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
      };

      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    },
    [width],
  );

  return {
    collapsed,
    width: collapsed ? COLLAPSED_WIDTH : width,
    dragging,
    toggle: () => setCollapsed((value) => !value),
    startResize,
    resetWidth: () => setWidth(DEFAULT_WIDTH),
  };
}

type Rect = { top: number; height: number };

export default function Sidebar({
  page,
  onSelect,
  engine,
  collapsed,
  width,
  dragging,
  onStartResize,
  onResetWidth,
}: {
  page: PageId;
  onSelect: (id: PageId) => void;
  engine: string | null;
  collapsed: boolean;
  width: number;
  dragging: boolean;
  onStartResize: (event: React.PointerEvent) => void;
  onResetWidth: () => void;
}) {
  const rows = useRef(new Map<PageId, HTMLDivElement | null>());
  const columnRef = useRef<HTMLDivElement | null>(null);
  const [pill, setPill] = useState<Rect | null>(null);
  const [pillReady, setPillReady] = useState(false);

  // Runs before paint, so the pill is already on the selected row on the first frame —
  // measuring after paint would show it flying in from the top-left corner.
  //
  // Both rects are viewport-relative and both come from the same frame, so their difference
  // is the offset inside the column regardless of how many positioning contexts sit between
  // them. `clientTop` is subtracted because absolute `top: 0` resolves against the padding
  // box while getBoundingClientRect reports the border box (today the column has no border,
  // but the arithmetic should not depend on that).
  const measure = useCallback(() => {
    const column = columnRef.current;
    const row = rows.current.get(page);
    if (!column || !row) return;
    const rect = row.getBoundingClientRect();
    const base = column.getBoundingClientRect();
    setPill({
      top: rect.top - base.top - column.clientTop,
      height: rect.height,
    });
  }, [page]);

  useLayoutEffect(() => {
    measure();
  }, [measure, collapsed]);

  // A window resize reflows everything above the rail, and the pinned row moves when the
  // engine hint wraps. Re-measure on any column size change rather than assuming 44px rows.
  useEffect(() => {
    const column = columnRef.current;
    if (!column || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => measure());
    observer.observe(column);
    return () => observer.disconnect();
  }, [measure]);

  // Enabling the transition only after the first measurement: otherwise the initial
  // placement animates from (0,0) and the app looks like it fumbled its first frame.
  useEffect(() => {
    if (pill === null || pillReady) return;
    const frame = requestAnimationFrame(() => setPillReady(true));
    return () => cancelAnimationFrame(frame);
  }, [pill, pillReady]);

  const register = useCallback((id: PageId) => {
    return (node: HTMLDivElement | null) => {
      rows.current.set(id, node);
    };
  }, []);

  return (
    <aside
      style={{ width }}
      className={cx(
        // `select-none` for the whole rail, not just while dragging: the rail is chrome, and a
        // drag that misses the 10px handle by two pixels used to select a paragraph of the page
        // instead of doing nothing. Text selection has no meaning here anyway.
        "relative flex shrink-0 flex-col pb-3 select-none",
        // No transition while dragging: a 180ms ease on every pointermove reads as lag.
        !dragging && "transition-[width] duration-280 ease-out",
      )}
    >
      <div
        ref={columnRef}
        className={cx(
          "relative flex h-full flex-col pt-1.5 transition-[padding] duration-280 ease-out",
          collapsed ? "px-2" : "px-2.5",
        )}
      >
        {pill ? (
          <span
            aria-hidden
            className={cx(
              "pointer-events-none absolute z-0 rounded-[10px] bg-ink",
              // Spring, not ease-out: the pill is a travelling object and the tiny overshoot
              // is what sells it. Disabled until the first placement (see above) and while the
              // rail is being dragged, where a lagging pill looks broken.
              !pillReady || dragging
                ? "transition-none"
                : "transition-[transform,left,right] duration-280 ease-spring",
            )}
            style={{
              top: 0,
              // Absolute positioning is measured from the padding box, so `inset-x-0` would
              // make the pill as wide as the rail and hang it past every row. These are the
              // rail's own gutter widths, kept in step with the column's padding above.
              left: collapsed ? 8 : 10,
              right: collapsed ? 8 : 10,
              height: pill.height,
              transform: `translateY(${pill.top}px)`,
            }}
          />
        ) : null}

        <nav className="relative z-10 flex flex-col gap-1">
          {NAV.filter((item) => !item.pinned).map((item, index) => (
            <NavRow
              key={item.id}
              rowRef={register(item.id)}
              item={item}
              active={page === item.id}
              collapsed={collapsed}
              index={index}
              onClick={() => onSelect(item.id)}
            />
          ))}
        </nav>

        <div className="relative z-10 mt-auto flex flex-col gap-1">
          {NAV.filter((item) => item.pinned).map((item) => (
            <NavRow
              key={item.id}
              rowRef={register(item.id)}
              item={item}
              active={page === item.id}
              collapsed={collapsed}
              index={0}
              hint={engine ? "已连接" : "未找到"}
              hintTone={engine ? "muted" : "warn"}
              onClick={() => onSelect(item.id)}
            />
          ))}
        </div>

        {/* Resize handle. Hidden while collapsed — there is nothing to resize. Drawn on
            demand only: the separator line between rail and content was removed on purpose,
            so a permanent 2px rule here would put it straight back. Double-click restores the
            default width, which is the fastest way back from a 184px rail. */}
        {!collapsed ? (
          <div
            onPointerDown={onStartResize}
            onDoubleClick={onResetWidth}
            role="separator"
            aria-orientation="vertical"
            aria-label="调整侧边栏宽度"
            tabIndex={0}
            title="拖动调整宽度 · 双击恢复默认"
            onKeyDown={(event) => {
              if (event.key === "ArrowLeft") {
                event.preventDefault();
                onResetWidth();
              }
            }}
            className="absolute top-0 -right-[5px] z-20 h-full w-[10px] cursor-col-resize"
          >
            <div
              className={cx(
                "mx-auto h-full w-[2px] transition-colors duration-120",
                dragging ? "bg-ink" : "bg-transparent hover:bg-line-strong",
              )}
            />
          </div>
        ) : null}
      </div>
    </aside>
  );
}

const NavRow = ({
  rowRef,
  item,
  active,
  collapsed,
  index,
  hint,
  hintTone = "muted",
  onClick,
}: {
  rowRef?: (node: HTMLDivElement | null) => void;
  item: NavItem;
  active: boolean;
  collapsed: boolean;
  index: number;
  hint?: string;
  hintTone?: "muted" | "warn";
  onClick: () => void;
}) => {
  const Icon = item.icon;
  const trailing = hint ?? item.hint;
  const [tooltip, setTooltip] = useState<null | "hover" | "press">(null);
  const hoverTimer = useRef<number | null>(null);

  // Hover is the mouse affordance, long-press the touch one. Both render the same label,
  // because the rail's whole cost is that the icon is the only clue to what a row does.
  const clearHover = useCallback(() => {
    if (hoverTimer.current !== null) {
      window.clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  }, []);

  const press = useLongPress(() => setTooltip("press"), 360);
  useEffect(() => clearHover, [clearHover]);

  return (
    <div
      ref={rowRef}
      className="relative animate-nav-in"
      style={{ animationDelay: `${index * 30}ms` }}
    >
      <button
        type="button"
        onClick={onClick}
        aria-current={active ? "page" : undefined}
        onPointerEnter={(event) => {
          if (!collapsed || event.pointerType !== "mouse") return;
          clearHover();
          hoverTimer.current = window.setTimeout(() => setTooltip("hover"), 240);
        }}
        onPointerLeave={() => {
          clearHover();
          setTooltip(null);
          press.onPointerLeave();
        }}
        onPointerDown={press.onPointerDown}
        onPointerMove={press.onPointerMove}
        onPointerUp={press.onPointerUp}
        onPointerCancel={press.onPointerCancel}
        onContextMenu={press.onContextMenu}
        className={cx(
          "group flex h-11 w-full items-center rounded-[10px]",
          // Only colour is transitioned here: the row sits on top of the moving pill and a
          // transform on the row would fight the pill's travel.
          "transition-colors duration-120 ease-out",
          "chrome overflow-hidden",
          collapsed ? "justify-center px-0" : "px-3",
          active
            ? "text-canvas"
            : "text-ink-soft hover:bg-surface-sunken hover:text-ink",
        )}
      >
        <Icon size={17} strokeWidth={2.4} className="shrink-0" />

        {/* Kept mounted while collapsed so it can fold away. Unmounting would make the label
            disappear on frame one and leave the icon jumping to centre afterwards. */}
        <span
          className={cx(
            "flex min-w-0 items-center overflow-hidden",
            "transition-[max-width,opacity,margin,transform] duration-280 ease-out",
            collapsed
              ? "ml-0 max-w-0 -translate-x-1 opacity-0"
              : "ml-3 max-w-[190px] translate-x-0 opacity-100",
          )}
        >
          <span className="text-[13.5px] font-semibold whitespace-nowrap">
            {item.label}
          </span>
          <span
            className={cx(
              "ml-auto pl-3 text-[11px] font-medium whitespace-nowrap",
              active
                ? "text-canvas/55"
                : hintTone === "warn"
                  ? "text-warn"
                  : "text-ink-faint",
            )}
          >
            {trailing}
          </span>
        </span>
      </button>

      {collapsed && tooltip ? (
        <span
          role="tooltip"
          className={cx(
            "pointer-events-none absolute top-1/2 left-[calc(100%+10px)] z-40 -translate-y-1/2",
            "animate-pop-in rounded-[8px] bg-ink px-2.5 py-1.5",
            "text-[12px] font-bold whitespace-nowrap text-canvas shadow-lg",
          )}
        >
          {item.label}
          <span className="ml-2 font-medium text-canvas/55">{trailing}</span>
        </span>
      ) : null}
    </div>
  );
};
