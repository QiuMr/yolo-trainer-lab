import { useEffect, useState } from "react";
import {
  ChevronsLeft,
  ChevronsRight,
  Copy,
  Minus,
  Search,
  Square,
  X,
} from "lucide-react";
import { isTauri } from "../lib/api";
import { cx } from "./ui";

/* ─────────────────────────────────────────────────────────────────────────────
   Custom title bar.

   The window is undecorated (tauri.conf.json → decorations: false) so the app can
   own its top edge: the system bar on Windows is 32px of grey that fights a white
   canvas. The cost is real — no DWM shadow edge and no Win11 snap-layouts popup on
   the maximize button — and is recorded in docs/DECISIONS.md.

   Dragging is delegated to Tauri via `data-tauri-drag-region`, which also handles
   double-click-to-maximize. We deliberately do not add our own dblclick handler:
   it would race the native one and cancel itself out.

   No product name here on purpose. The app has no chrome to speak of, and a name in
   the top-left of an otherwise empty white bar just repeats what the taskbar says.
   The sidebar toggle lives in that corner instead — the one control that has to be
   findable before anything else is on screen.
   ───────────────────────────────────────────────────────────────────────────── */

type WindowHandle = {
  minimize: () => Promise<void>;
  toggleMaximize: () => Promise<void>;
  close: () => Promise<void>;
  isMaximized: () => Promise<boolean>;
  onResized: (handler: () => void) => Promise<() => void>;
};

function useWindow(): WindowHandle | null {
  const [handle, setHandle] = useState<WindowHandle | null>(null);

  useEffect(() => {
    if (!isTauri) return;
    let alive = true;
    // Imported lazily: the module throws in a plain browser, and the preview build must still run.
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      if (alive) setHandle(getCurrentWindow() as unknown as WindowHandle);
    });
    return () => {
      alive = false;
    };
  }, []);

  return handle;
}

export default function TitleBar({
  collapsed,
  width,
  dragging,
  onToggle,
  onOpenPalette,
}: {
  collapsed: boolean;
  width: number;
  dragging: boolean;
  onToggle: () => void;
  onOpenPalette: () => void;
}) {
  const win = useWindow();
  const [maximized, setMaximized] = useState(false);

  // The restore icon is only correct if it follows the real window state, including changes made
  // by dragging the window to a screen edge or by Win+Up.
  useEffect(() => {
    if (!win) return;
    let dispose: (() => void) | undefined;
    const sync = () => void win.isMaximized().then(setMaximized);

    sync();
    void win.onResized(sync).then((off) => {
      dispose = off;
    });
    return () => dispose?.();
  }, [win]);

  return (
    <header className="flex h-10 shrink-0 items-center select-none">
      {/* The toggle sits in its own column of the sidebar's width, so it stays in the same
          vertical line as the nav icons below it while the rail is animating. */}
      <div
        style={{ width }}
        className={cx(
          "flex h-full shrink-0 items-center",
          dragging ? "select-none" : "transition-[width] duration-280 ease-out",
          collapsed ? "justify-center" : "pl-2.5",
        )}
      >
        <button
          type="button"
          onClick={onToggle}
          aria-label={collapsed ? "展开侧边栏" : "收起侧边栏"}
          title={`${collapsed ? "展开" : "收起"}侧边栏 · Ctrl+B`}
          className={cx(
            "press flex h-8 w-8 items-center justify-center rounded-[9px] text-ink-soft",
            "transition-colors duration-120 ease-out",
            "hover:bg-surface-sunken hover:text-ink",
          )}
        >
          {/* Two chevrons cross-fading through a quarter turn, rather than swapping one icon
              for another. The rotation is what reads as "folding", and it keeps the two states
              visually continuous instead of looking like the button changed its mind. */}
          <span className="relative flex h-4 w-4 items-center justify-center">
            <ChevronsLeft
              size={17}
              strokeWidth={2.1}
              className={cx(
                "absolute transition-[opacity,transform] duration-180 ease-out",
                collapsed ? "rotate-90 opacity-0" : "rotate-0 opacity-100",
              )}
            />
            <ChevronsRight
              size={17}
              strokeWidth={2.1}
              className={cx(
                "absolute transition-[opacity,transform] duration-180 ease-out",
                collapsed ? "rotate-0 opacity-100" : "-rotate-90 opacity-0",
              )}
            />
          </span>
        </button>
      </div>

      {/* Only the empty middle is a drag region — putting it on <header> would make the
          toggle button unclickable, because a drag region swallows the press. */}
      <div data-tauri-drag-region className="h-full flex-1" />

      <div className="flex h-full items-stretch">
        {/* The palette lives next to the window controls, where every editor puts it,
            instead of floating over the canvas where it would sit on top of content. */}
        <button
          type="button"
          onClick={onOpenPalette}
          aria-label="命令面板"
          title="命令面板 · Ctrl+K"
          className="press flex w-10 items-center justify-center text-ink-faint transition-colors duration-120 ease-out hover:bg-surface-sunken hover:text-ink"
        >
          <Search size={14} strokeWidth={2.4} />
        </button>
        <WindowButton
          label="最小化"
          onClick={() => void win?.minimize()}
          icon={Minus}
        />
        <WindowButton
          label={maximized ? "还原" : "最大化"}
          onClick={() => void win?.toggleMaximize()}
          icon={maximized ? Copy : Square}
        />
        <WindowButton
          label="关闭"
          danger
          onClick={() => void win?.close()}
          icon={X}
        />
      </div>
    </header>
  );
}

function WindowButton({
  label,
  icon: Icon,
  onClick,
  danger = false,
}: {
  label: string;
  icon: typeof X;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cx(
        "press flex w-12 items-center justify-center text-ink-soft",
        "transition-colors duration-120 ease-out",
        danger
          ? "hover:bg-danger hover:text-canvas"
          : "hover:bg-surface-sunken hover:text-ink",
      )}
    >
      <Icon size={14} strokeWidth={2.2} />
    </button>
  );
}
